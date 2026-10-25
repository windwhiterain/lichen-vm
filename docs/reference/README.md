# Reference material

Papers and documents this project reads. Each entry names the source it was
taken from, so a reader can check the original against any transcription here.

## Vertex Block Descent

Anka He Chen, Ziheng Liu, Yin Yang, Cem Yuksel. *Vertex Block Descent*.
ACM Transactions on Graphics 43(4), Article 116, SIGGRAPH 2024.

- [Vertex Block Descent.pdf](Vertex%20Block%20Descent.pdf) — the paper as
  published, downloaded from [arXiv:2403.06321](https://arxiv.org/abs/2403.06321)
  (v4, 1 Jun 2024). The published version is
  [doi:10.1145/3658179](https://doi.org/10.1145/3658179).
- [Vertex Block Descent.md](Vertex%20Block%20Descent.md) — the same paper as
  Markdown, with its mathematics as LaTeX and its figures under
  [figures/vbd/](figures/vbd/).

The method: a block coordinate descent solver for the variational form of
implicit Euler, run as vertex-level Gauss-Seidel iterations — one vertex block
at a time, with the mesh coloured so that a whole colour updates in parallel.
Each local solve is a 3x3 system, so the per-vertex cost stays small while the
global energy decreases monotonically, which is what makes a single iteration
per time step stable. It is here as reference material for the solver side of
the project, not as a citation of lichen's own design.

### How the Markdown was produced

The Markdown is generated from the paper's own LaTeX source — the arXiv
e-print, `https://arxiv.org/e-print/2403.06321` — because that source is what
the published PDF was compiled from, and it carries the mathematics as LaTeX
rather than as positioned glyphs.

A PDF has no mathematical structure: the numerator of a fraction is drawn
above the denominator, and the text layer stores the two as separate runs. A
text extractor therefore cannot rebuild `1/(2h^2)`, and the transcription of
such a PDF loses the fraction, the exponents, and often the reading order
between the two columns. The LaTeX source has all of that explicitly.

The pipeline is `pandoc` for the document structure, with the parts pandoc's
LaTeX reader cannot carry handled afterwards:

| Part | How it was made |
| --- | --- |
| Text, sections, inline math | `pandoc` from the section sources |
| Display mathematics | `pandoc`'s `align` and `equation` environments; every equation is verified verbatim against the source |
| Figures | the 58 image files from the e-print, cropped as the paper crops them, scaled to 1200 px, and the nine vector diagrams rasterised at 150 dpi |
| Table 1 | the `tabular` source, parsed into a Markdown table |
| Algorithm 1 | the `algorithm2e` source, parsed into an indented code block |
| Citations and references | the ACM author-year style of `main.bbl`, with the bibliography in the paper's order |

Two things in the source have no counterpart in the rendered paper and are
dropped: the tracked-change markers (`\del{…}` removes text, `\add{…}` keeps
it) and the authors' comments.

The paper's `\Vec{}` macro renders as ordinary italic mathematics — the
published PDF contains no bold or arrow font — so it is transcribed as plain
italic. The equation numbers, the figure numbers and the cross references
("Equation 8", "Figure 19a") are the paper's own.

Read the PDF when the typeset layout, the figures' original resolution, or
the exact wording of a caption matters; read the Markdown for the text and
the mathematics.

A text-extraction converter is also installed on this machine for PDF work:
Firecrawl's open-source [`anydoc`](https://github.com/firecrawl/anydoc) CLI
0.2.4, at `C:\resource\tools\pdf2md` (`anydoc.cmd` and `anydoc` are the
wrappers), driven by the portable Node.js 24.21.0 runtime in
`C:\resource\tools\node`. It is a fast, general PDF-to-Markdown tool, and it
is what a plain text extraction of this paper produces — which is why this
paper's Markdown is not built from it: the mathematics does not survive a
text extraction.

