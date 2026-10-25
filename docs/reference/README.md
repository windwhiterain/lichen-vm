# Reference material

Papers and documents this project reads. Each entry names the source it was
taken from, so a reader can check the original against any transcription here.

## Vertex Block Descent

Anka He Chen, Ziheng Liu, Yin Yang, Cem Yuksel. *Vertex Block Descent*.
ACM Transactions on Graphics 43(7), SIGGRAPH 2024.

- [Vertex Block Descent.pdf](Vertex%20Block%20Descent.pdf) — the paper as
  published, downloaded from [arXiv:2403.06321](https://arxiv.org/abs/2403.06321)
  (v4, 1 Jun 2024). The published version is
  [doi:10.1145/3658179](https://doi.org/10.1145/3658179).
- [Vertex Block Descent.md](Vertex%20Block%20Descent.md) — the same paper as
  Markdown, converted from the PDF with the Firecrawl `anydoc` CLI 0.2.4.

The method: a block coordinate descent solver for the variational form of
implicit Euler, run as vertex-level Gauss-Seidel iterations — one vertex block
at a time, with the mesh coloured so that a whole colour updates in parallel.
Each local solve is a 3x3 system, so the per-vertex cost stays small while the
global energy decreases monotonically, which is what makes a single iteration
per time step stable. It is here as reference material for the solver side of
the project, not as a citation of lichen's own design.

### How the Markdown was produced

The converter is Firecrawl's open-source document-to-Markdown tool, `anydoc`
([firecrawl/anydoc](https://github.com/firecrawl/anydoc)), used through its
command-line entry point:

```
anydoc "Vertex Block Descent.pdf" -o "Vertex Block Descent.md"
```

On this machine the CLI is installed outside the repository, at
`C:\resource\tools\pdf2md` — the npm package `@firecrawl/anydoc` 0.2.4 with
its prebuilt Windows native addon, driven by the portable Node.js 24.21.0
runtime in `C:\resource\tools\node`. `anydoc.cmd` and `anydoc` in that
directory are the wrappers.

The conversion extracts text, so it is a transcription rather than a
re-typeset paper:

- **Kept**: the full body text, the section numbering, the figure and table
  captions, and the bibliography with its DOI links.
- **Lost**: the page layout. Running headers and author lines appear inline in
  the text, the two-column reading order is not recovered, and words split
  across a line break keep the hyphen ("reduc- tions"). Equations survive as
  inline fragments of text rather than as LaTeX, figures are absent (only
  their captions remain), and wide tables keep their cell text but lose their
  layout.

Read the PDF when the exact wording, the typeset mathematics, or the figures
matter; read the Markdown when the text is what is wanted.
