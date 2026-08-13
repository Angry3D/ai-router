# Pricing catalog provenance

<!-- provenance:openai-pricing-snapshots -->

The files in this directory are dated factual-data snapshots derived from the
official OpenAI pricing documentation linked by each JSON file. They do not
copy upstream code or explanatory prose.

`openai-standard-2026-07-27.json` was captured on 2026-07-27.
`openai-priority-2026-07-28.json` was captured on 2026-07-28. Values use the
integer unit `micro_usd_per_million_tokens`; runtime totals are persisted as
pico-USD to avoid floating-point money arithmetic.

These snapshots are local estimates, not an OpenAI bill, quote, or promise
that the rates remain current. Review the linked official documentation and
the actual provider bill before relying on the values. A catalog update must
use a new dated version, preserve source URLs and capture date, and update the
license/provenance audit binding.
