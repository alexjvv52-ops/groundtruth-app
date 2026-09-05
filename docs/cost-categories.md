# Canonical cost categories

**Authority:** BOOKS-BOUNDARY §2 / §4 · ROADMAP Track 3  
**Status:** shipped as structured data in `src-tauri/src/categories.rs`

Farm OS makes no tax determination. It does not choose between Schedule F and Schedule C, does not decide capital versus expense, and computes nothing. It records what the operator says and carries both mappings so the preparer never re-types.

The operator picks the plain-language name. Line numbers never appear in the UI.

No category carries an amount, a rate, a default price, or a per-unit cost.

| name | F line | C line | descriptor required |
|------|--------|--------|---------------------|
| Seed | 26 | 22 | no |
| Growing medium | 28 | 22 | no |
| Trays, domes and racks | 28 | 22 | no |
| Packaging and labels | 28 | 22 | no |
| Cleaning and sanitizing | 11 | 22 | no |
| Nutrients and amendments | 17 | 22 | no |
| Electricity, water and heat | 30 | 25 | no |
| Grow space rent | 24b | 20b | no |
| Repairs and maintenance | 25 | 21 | no |
| Inbound freight and shipping | 18 | 22 | no |
| Delivery fuel | 19 | 9 | no |
| Insurance | 20 | 15 | no |
| Tools and small equipment | 28 | 22 | no |
| Market stall or booth fee | 32 other | 27b other | yes |
| Advertising and printing | 32 other | 8 | yes |
| Website, POS and payment fees | 32 other | 27b other | yes |
| Professional fees | 32 other | 17 | yes |
| Office and admin supplies | 32 other | 18 | yes |

Schedule F has no advertising, office, legal or travel line — those fall to 32a–32f "Other expenses (specify)", which requires a written description. That is why the last five require a descriptor. Schedule C names them individually, which is why both mappings are carried.
