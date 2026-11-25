---
default: minor
---

# `dyno verify`: check a lab's signed receipt for a result

A result can come from a lab: someone who owns real phones and runs dyno for people who do not. A lab now signs a small receipt for each result, and `dyno verify` checks it, so that you can trust the lab's phone readings for exactly the files you hold, without running the check again:

```sh
dyno verify receipt.json --keys https://lab.example/keys.json --result result.json --build ./dist
```

It checks that a public key you gave signed the receipt (`--key`, or the lab's keys document with `--keys`), that the result file is the one the receipt is for (`--result`), and that your files have the receipt's build id (`--build`), with the code that `dyno check` uses. It writes a JSON verdict and exits with 0 when the receipt is verified and 1 when it is not. With no key it refuses: a receipt that brought its own key would prove nothing. It also refuses a receipt, a result or a keys document in which an object has one name twice, because parsers differ in which value they keep.

The receipt format is open, and anyone can run a lab. `dyno receipt keygen` makes a lab's Ed25519 key pair, and `dyno receipt sign` signs the receipt for a result of a check with a run on a device. `docs/receipts.md` gives the format, version 1, and a known-answer receipt in `fixtures/receipts/` for other implementations to test against.

`@dyno-cloud/schema` has the new types `ReceiptEnvelope`, `Receipt`, `PublicKeys` and `VerifyResult`, and the JSON Schema files `receipt.schema.json`, `receipt-keys.schema.json` and `verify.schema.json`.
