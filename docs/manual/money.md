# Money

Money is the register: wholesale, leftover, cash in, cash out. It is
not Books (Books is a lens on this same register). Unpaid stays
unpaid. Unpriced stays unpriced. A payment link is not a payment.

## If something is wrong

- Cash collected on Books is lower than you expect and Health is
  green -> you are in a period. Open **This week** / **This month**
  / **All time** on Books. Do not press **Paid…** again.
- Delivered is dead and a refusal sentence sits under the row ->
  read that sentence. Do not void the order to force a delivery.
- Paid… will not Save on an unpriced row -> price the lines first.
  Do not type a round number to skip the price.
- A Stripe fact sits under **Stripe money not recorded** -> nothing
  was applied. Use **Paid…** or **Reverse payment** as the sentence
  says. Do not invent a second income row to match it.
- Leftover List leftover refuses -> one listing per crop-day, and
  ounces cannot exceed that day's harvest. Do not list a second time.
- Connect Stripe Continue is dead -> the Restricted key field is
  empty. Do not paste a live key. Test mode only.

## Doors on this tab

### Wholesale

- **Invoice** - writes nothing - opens the bill.
- **Delivered** - `wholesale.delivered`.
- **Void** then **Confirm void** - `wholesale.voided`.
- **Paid…** then **Save** / **Record anyway** - `income.received` and
  `wholesale.paid`; plus `wholesale.write_off` when you settle short.
- **Payment link** - `wholesale.link_minted` - mints the link. Moves
  no money.
- **Copy** - writes nothing.
- **Write off as bad debt** then **Write it off as bad debt** -
  `wholesale.bad_debt`. **Keep it owed** writes nothing.
- **Reverse payment** then **Reverse the payment** -
  `wholesale.payment_reversed`. **Keep it paid** writes nothing.
- **Back to Today** - writes nothing.

### To collect

Heading only. The converting taps are on the row.

### Connect Stripe / Replace key

Writes nothing. The key is config on this machine, not an event.

- **Continue** - preview only.
- **Connect this account** - stores the test key.
- **Use a different key** / **Cancel** - writes nothing.

![Money, Connect Stripe](../images/money/money-stripe-connect.png)

### Invoice bill

- **Print** / **Close** - writes nothing.

![Money, invoice](../images/money/money-invoice-creation.png)

### New order

- **Same as last order** - writes nothing - fills the draft.
- **Add variety** - writes nothing.
- **Record order** / **Record anyway** - `wholesale.ordered`.

![Money, New order](../images/money/money-new-order.png)

![Money, order recorded](../images/money/money-new-order-created.png)

### Leftover

- **List leftover** - `leftover.listed`.
- **Invoice** - writes nothing.
- **Payment link** - `leftover.link_minted`.
- **Paid…** then **Save** - `income.received` and `leftover.paid`.
- **Copy** - writes nothing.
- **Write shop page** - writes nothing - a file on disk, not an
  event. Shop remount is refused. See [refused](refused.md).
- **Open folder** - writes nothing.

![Money, leftover](../images/money/money-list-left-over.png)

### Cash in

- **Apply to an order…** then **Apply** / **Apply anyway** -
  `wholesale.paid`; plus `wholesale.write_off` when short. **Cancel**
  writes nothing.

### Cash out

- **Correct** then **Save correction** - `cost.money_out_corrected`.
- **Void** then **Void** on the sheet - `cost.money_out_voided`.

### Expense corrections

A trail. No door.

### Stripe money not recorded

A list of facts that did not apply. No door.

### Record money

The same six rows as Today.

![Money, Record money](../images/money/money-record-money.png)

## Figures

![Money, first open](../images/money/money1.png)

![Money, the register](../images/money/money2.png)
