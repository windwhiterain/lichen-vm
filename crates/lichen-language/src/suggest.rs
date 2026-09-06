//! Did-you-mean suggestions for an unresolved name, shared by the lowering
//! compiler ([`crate::compile`]) and the incremental session ([`crate::session`]).
//!
//! Both resolver paths report an *unresolved name* as a `Resolve` diagnostic.
//! When the typo is close to a name that is actually in scope, the diagnostic
//! names the candidate(s) so the editor can suggest a fix and power completion.
//! The heuristic is deliberately conservative: it only suggests a name that is
//! close by edit distance, and it refuses to suggest for a one-character typo
//! (where any one-character name would be "a match") unless the two share a
//! first character.

/// Damerau–Levenshtein (optimal-string-alignment) edit distance between `a` and
/// `b`: the minimum number of insertions, deletions, substitutions, and
/// *adjacent transpositions* to turn `a` into `b`.  The transposition rule is
/// what makes a common typo like `unkown` → `unknown` distance 1, which plain
/// Levenshtein would report as 2.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let n = a.len();
    let m = b.len();
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    // d[i][j] = distance between a[..i] and b[..j].
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..=n {
        d[i][0] = i;
    }
    for j in 0..=m {
        d[0][j] = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

/// Whether `cand` is close enough to the typo `name` to be worth suggesting.
///
/// The distance threshold scales with the typo's length (so a long name allows
/// a couple of edits), and a very short typo must share its first character
/// with the candidate — otherwise every one-character name would match an
/// arbitrary one-character typo and the suggestions would be noise.
fn close(name: &str, cand: &str) -> bool {
    let dist = edit_distance(name, cand);
    if dist == 0 {
        return false; // an exact match would have resolved; never suggest it.
    }
    let len = name.chars().count();
    let max_dist = (len / 3).max(1);
    if dist > max_dist {
        return false;
    }
    if len < 3 && name.chars().next() != cand.chars().next() {
        return false;
    }
    true
}

/// The in-scope names closest to the unresolved `name`, best-first and capped
/// at 3.  `in_scope` may contain duplicates (a name visible in several frames);
/// the result is deduplicated and ordered deterministically (by edit distance,
/// then by name) so a test and the message are stable.  Returns `None` when no
/// name is close enough.
pub fn suggest_names<'a>(
    name: &str,
    in_scope: impl IntoIterator<Item = &'a str>,
) -> Option<Vec<&'a str>> {
    let mut hits: Vec<(usize, &'a str)> = in_scope
        .into_iter()
        .filter(|cand| close(name, cand))
        .map(|cand| (edit_distance(name, cand), cand))
        .collect();
    if hits.is_empty() {
        return None;
    }
    // Stable sort by (distance, name): ties break alphabetically.
    hits.sort_by_key(|(dist, cand)| (*dist, *cand));
    hits.dedup_by_key(|(_, cand)| *cand);
    hits.truncate(3);
    Some(hits.into_iter().map(|(_, cand)| cand).collect())
}

/// The did-you-mean clause for a name typed against a set of valid names, or
/// `None` when nothing is close.  The clause is `, did you mean 'y'?` (one
/// candidate) or `, did you mean one of 'a', 'b'?` (several).  Shared by the
/// unresolved-name and the field-access messages so both read identically.
pub fn did_you_mean<'a>(name: &str, in_scope: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let cands = suggest_names(name, in_scope)?;
    Some(if cands.len() == 1 {
        format!(", did you mean '{}'?", cands[0])
    } else {
        let joined = cands
            .iter()
            .map(|c| format!("'{c}'"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(", did you mean one of {joined}?")
    })
}

/// Render the full `unresolved name` diagnostic message for `name`, appending a
/// did-you-mean clause when a close in-scope name exists.  `in_scope` yields the
/// names visible at the use site (duplicates are fine).  No candidate → the
/// plain `unresolved name 'x'` message (so an error with nothing to suggest is
/// unchanged).
pub fn unresolved_message<'a>(name: &str, in_scope: impl IntoIterator<Item = &'a str>) -> String {
    let base = format!("unresolved name '{name}'");
    match did_you_mean(name, in_scope) {
        Some(clause) => format!("{base}{clause}"),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance_handles_insert_delete_substitute_and_transpose() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("a", ""), 1);
        assert_eq!(edit_distance("", "a"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        // A common typo: adjacent transposition counts as a single edit.
        assert_eq!(edit_distance("unkown", "unknown"), 1);
        assert_eq!(edit_distance("ac", "ca"), 1);
    }

    #[test]
    fn one_character_typos_need_a_shared_first_character() {
        // `y` vs `x` / `a` differ by one char and share no first char: no
        // suggestion (any one-char name would otherwise match).
        assert!(suggest_names("y", ["x", "a"]).is_none());
        // An exact match is never suggested (it would have resolved).
        assert!(suggest_names("x", ["x"]).is_none());
    }

    #[test]
    fn a_close_typo_is_suggested() {
        assert_eq!(suggest_names("unkown", ["unknown"]), Some(vec!["unknown"]));
    }

    #[test]
    fn no_candidate_returns_none() {
        assert!(suggest_names("zzz", ["add", "sub"]).is_none());
    }

    #[test]
    fn suggestions_are_deduplicated_and_capped() {
        let got = suggest_names("a", ["au", "au", "ab", "ac", "ad", "ae"]).expect("close names");
        assert_eq!(got.len(), 3, "capped at 3, got {got:?}");
        assert_eq!(got[0], "ab", "best (distance) first");
    }
}
