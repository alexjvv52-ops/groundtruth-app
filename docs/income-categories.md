# Canonical income categories

**Authority:** BOOKS-BOUNDARY (amended) · money-in track  
**Status:** proposed and UNCONFIRMED — structured data in `src-tauri/src/categories.rs`

These mappings are proposed and UNCONFIRMED. The operator must have them checked by a tax preparer before the first real record. Farm OS makes no tax determination. It does not choose between Schedule F and Schedule C, does not decide which line a payment belongs on, and computes nothing. It records what the operator says and carries both mappings so the preparer never re-types.

The operator picks the plain-language name. Line numbers never appear in the UI.

No category carries an amount, a rate, a default price, or a per-unit figure.

| name | F line | C line | descriptor required |
|------|--------|--------|---------------------|
| Produce you grew | 2 | 1 | no |
| Goods you bought to resell | 1b | 1 | no |
| Custom work or hire | 7 | 1 | no |
| Agricultural program payment | 4b | 6 other | yes |
| Crop insurance or disaster payment | 6a | 6 other | yes |
| Other farm income | 8 other | 6 other | yes |

Schedule C line 6 and Schedule F line 8 are "other" lines that require a written description. That is why the last three require a descriptor. Both mappings are carried so the preparer never re-types.
