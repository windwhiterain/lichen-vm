#[macro_export]
macro_rules! tokens {
    ($(#[$($attr:tt)*])? $name:ident {$($token:tt)*}) => {
        $crate::tokens!{@internal {$} $(#[$($attr)*])? $name {$($token)*}}
    };
    (@internal {$d:tt} $(#[$($attr:tt)*])? $name:ident {$($token:tt)*}) => {
        $(#[$($attr)*])?
        macro_rules! $name{
            ($macro:path {$d($pre_token:tt)*}{$d($post_token:tt)*})=>{
                $macro!{$d($pre_token)* {$($token)*} $d($post_token)*}
            }
        }
    };
}

#[macro_export]
macro_rules! group {
    ($(#[$($attr:tt)*])? $name:ident {$($tokens:path $(: $field:ident)?,)*}) => {
        $crate::group!{@internal $(#[$($attr)*])? $name {$($tokens $(: $field)?,)*} {}}
    };
    (@internal $(#[$($attr:tt)*])? $name:ident {} {$($($group_field:ident)?{$($group:tt)*})*} $($($new_field:ident)?{$($token:tt)*})?) => {
        $crate::tokens!{$(#[$($attr)*])? $name {$($($group_field)?{$($group)*})* $($($new_field)?{$($token)*})?}}
    };
    (@internal $(#[$($attr:tt)*])? $name:ident {$tokens:path $(: $field:ident)?,$($tokens_:path $(: $field_:ident)?,)*} {$($($group_field:ident)?{$($group:tt)*})*} $($($new_field:ident)?{$($token:tt)*})?) => {
        $tokens!{$crate::group {@internal $(#[$($attr)*])? $name {$($tokens_ $(: $field_)?,)*} {$($($group_field)?{$($group)*})* $($($new_field)?{$($token)*})?}$($field)?} {} }
    };
}

#[test]
fn test() {
    mod test {
        macro_rules! m {
            ({$($module:ident{$($x:ident)*})*}) => {
                $(
                    mod $module{
                        $(
                            #[allow(non_upper_case_globals)]
                            pub const $x: usize = 1;
                        )*
                    }
                )*
            };
        }
        tokens! {abc {a b c}}
        tokens! {def {d e f}}
        group! {abc_def {abc:abc,def:def,}}
        //const K: &str = abc_def!(stringify {} {});
        #[allow(dead_code)]
        fn test() {
            abc_def! {m {} {}}
            let _ = abc::b;
            let _ = def::e;
        }
    }
}
