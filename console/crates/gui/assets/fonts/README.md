# Embedded typefaces

- **Cormorant Garamond Italic**, Christian Thalmann / The Cormorant Project Authors.
  Source: https://github.com/google/fonts/tree/main/ofl/cormorantgaramond
  Used for the small MECHREVO wordmark. Page headings use the normal interface
  family, including the operating system's Chinese fallback.
- **Manrope**, Mikhail Sharanda / The Manrope Project Authors.
  Source: https://github.com/google/fonts/tree/main/ofl/manrope
  Used for Latin interface text and temperature units. Temperature and fan-speed
  values use egui's monospace family so equal-length readings keep a stable width;
  Manrope's proportional digits are not treated as tabular figures.

Both original variable TTFs are distributed under the SIL Open Font License 1.1.
The corresponding full licenses are included beside the fonts. Chinese text
continues to use the operating system's font provider; the application does not
download fonts at runtime. The fonts are embedded in both platform binaries.
