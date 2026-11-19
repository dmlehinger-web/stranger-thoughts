# Privacy draft

This is a draft for review before any deployed environment collects a real email, mailing address, or descriptor. The same draft is on the v2 site at `frontend/privacy/`. It is not a finished legal policy.

## What is collected

A writer account needs an email and a pseudonym. Optional descriptors are an age range, a coarse region, race, and an economic-status range. They are stored only if the writer chooses them. They are not used to choose a match. The writer can delete them without deleting the account.

Age ranges start at 18. The service rejects under-13 and other child buckets and does not offer them in the form.

A reader account needs an email, a pseudonym, and a mailing address so the writer matched to them can send a letter. The form recommends a PO box or a virtual mailbox. A street address is accepted and treated as highly sensitive personal information.

## Who can read a mailing address

The address is encrypted with a customer-managed key before it is stored. The API returns it only to the writer who is currently matched to that reader, and only for that match id. Letter lists, older matches, logs, and error bodies do not include it. After a rotate, the previous writer can no longer read it.

## Payments

Tips use Stripe Checkout and Stripe Connect. Card numbers are entered on Stripe’s pages. Connect Express identity checks are Stripe’s. The build is test mode until a later review allows live keys.

## Deletion

Deleting an account removes the profile, the optional descriptors, and the encrypted address. Match rows that the other person still needs keep a match id and a time, and the pseudonym becomes “Removed”. Tip rows keep ids and amounts so a payment can be reconciled. Point-in-time recovery can hold ciphertext for up to 35 days.

The inventory of fields, where they live, and who can read them is in [SECURITY-REVIEW.md](SECURITY-REVIEW.md).
