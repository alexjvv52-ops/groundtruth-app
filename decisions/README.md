# Decisions

Architectural and operational decision records for this project are maintained
privately by the project operator and are not published in this repository.

## Money-path boundary

Certain areas of this codebase are governed by a boundary contract and must not
be modified without explicit operator sign-off:

- The sales flow: checkout session creation, payment status polling, and the
  confirm-then-consume sequence.
- Consumption logic and any code that decrements or reconciles inventory.
- The database schema and all migrations.

Pull requests touching these areas will not be merged without prior discussion.
Please open an issue describing the intended change before writing code.

Everything else in this repository is open to contribution under the terms of
the Apache-2.0 licence in LICENSE.
