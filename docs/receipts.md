# Receipts: a lab's signed word for a result

A result of `dyno check` can come from a lab: someone who owns real phones and runs dyno for people who do not. A receipt lets you trust a lab's phone readings for exactly the files you hold, without running the check again.

A lab signs a small receipt for each result. You work out the build id of your files with dyno, and check the receipt's signature against the lab's public key. `dyno verify` does both.

The format is open and so is the code. Anyone can run a lab and sign with a key of their own: see [Run a lab](#run-a-lab). A lab's private key is the lab's own, and dyno never sends it anywhere.

A result's own `result_hash` cannot do this job. It covers only the deterministic part of a result, and the phone readings are in the run part. Also, a hash says that a file did not change, not who wrote it.

## What a verified receipt tells you

- The holder of the lab's private key signed this receipt, and nobody changed it since.
- With `--result`: the result file you hold is the one the lab signed, in every field.
- With `--build`: the files you hold are the files the lab ran.

It does not tell you that the lab is honest, or that its phone was set up well. You choose which labs to trust when you choose which public keys to give.

A receipt with no `--build` check is the lab's word about some build. Give `--build` to know that it is about yours.

## `dyno verify <RECEIPT>` (Phase 3)

```sh
dyno verify receipt.json --keys https://lab.example/keys.json --result result.json --build ./dist
```

| Flag                 | What it does                                                                                                                       |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `--key <KEY>`        | A public key of the lab that you trust: its raw 32 bytes in base64url with no padding. Give the flag again for each more key.      |
| `--keys <FILE\|URL>` | The lab's [public keys document](#a-labs-public-keys): a file, or an https URL. It also says when the lab retired each key.        |
| `--result <FILE>`    | Also check that this result file is the one the receipt is for.                                                                    |
| `--build <BUILD>`    | Also work out the build id of this build folder or .zip, with the code that `dyno check` uses, and check that it is the receipt's. |
| `--out <FILE>`       | Write the JSON verdict to this file instead of stdout.                                                                             |

You must give `--key` or `--keys`, or both. With neither, verify refuses with a usage error. A receipt names its key by id and never brings the key itself, because a receipt that brought its own key would prove nothing: anyone can sign anything with a key they made.

Get the lab's key from the lab, by a way you trust, such as its own site over https. verify takes a keys URL over https only, because over plain http anyone on the network can swap the keys. Plain http works only for a URL on your own machine, such as `http://localhost:8000/keys.json`. verify opens no connection unless `--keys` is a URL.

The two kinds of URL get different care:

- An https URL can redirect up to 3 times, and only to https. Its request can go through a proxy that your environment names, because TLS still ends at the lab's own host.
- A plain http URL follows no redirect and uses no proxy, and its request goes to a loopback address that dyno itself chose. For `localhost`, dyno looks the name up and refuses it when any of its addresses is not this machine. So neither the request nor its answer can cross a network.
- Either way, the request may take 30 seconds, and the document may have 1 MB.

### The checks

verify makes these checks in this order. The verdict lists every one, with `match`, `mismatch` or `not_checked`.

| Check             | It matches when                                                                                                                                                                                            |
| ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `key`             | One of the keys you gave has the receipt's `key_id`. A key with another key id is never tried.                                                                                                             |
| `signature`       | The signature is that key's Ed25519 signature of the receipt.                                                                                                                                              |
| `key_not_retired` | The lab had not retired the key when it issued the receipt: the key has no `retired_at`, or the receipt's `issued_at` is before it.                                                                        |
| `result_sha256`   | With `--result`: the SHA-256 of the result file's canonical JSON is the receipt's `result_sha256`.                                                                                                         |
| `result_build_id` | With `--result`: the result's `deterministic.build.build_id` is the receipt's `build_id`.                                                                                                                  |
| `result_hash`     | With `--result`: the result's `result_hash` is the receipt's. A result with none, or with null, matches a receipt's null. One that is not text never matches.                                              |
| `build_id`        | With `--build`: the build id of the files is the receipt's `build_id`. A .zip with two entries of one name has no build id: it is an input error (64), because programs differ in which entry they unpack. |

A check that no flag asked for is `not_checked`. So are the checks after `signature` when no key signed the receipt, because its values are then nobody's claim.

A retired key still verifies a receipt that was issued before its `retired_at`, and the output says that the key is retired. A key given with `--key` has no times, so alone it is never retired. When the same key comes with `--key` and in the `--keys` document, the document's times count, whichever flag is first: `--key` cannot hide a retirement.

### The output

The verdict is canonical JSON on stdout, or in the file that `--out` names. `packages/schema/json/verify.schema.json` and the type `VerifyResult` describe it.

| Field            | What it holds                                                                                                                                                                 |
| ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `verified`       | True when a key you gave signed the receipt, and no check is a `mismatch`. The exit code comes from it alone.                                                                 |
| `reason`         | Why the receipt is not verified: the reason of the first check that did not match. Null when it is verified.                                                                  |
| `checks`         | Each check: its `id`, its `outcome`, `expected` (what the receipt states), `actual` (what dyno found) and `reason` (why it did not match, or was not made).                   |
| `key`            | The key that signed: `key_id`, `created_at`, `retired_at` and `retired`. Null when no key you gave signed the receipt.                                                        |
| `receipt`        | What the lab signed: the issuer, the key id, the time, the device and the measures. Null unless the signature matched, so that dyno never repeats a claim that nobody signed. |
| `tool`           | The dyno version that verified.                                                                                                                                               |
| `schema_version` | The version of the verdict's shape.                                                                                                                                           |

The verdict holds no time and no machine detail. The same receipt, keys, result and build always give the same bytes, from the same dyno version.

The summary on stderr says `verified` or `not verified` and why, then what the lab signed, then each check. It has no stable format.

### Exit codes

| Code | Meaning                                                                                                                                                 |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0    | Verified.                                                                                                                                               |
| 1    | Not verified: no key you gave signed the receipt, the key was retired, or the result or the build is not the one the receipt is for.                    |
| 64   | A usage or input error: no key given, or a receipt, a key, a keys document, a result or a build that cannot be read or is not one. There is no verdict. |
| 70   | The keys document's URL cannot be reached or answers with an error, or the verdict cannot be written.                                                   |

A file that is not a version 1 receipt is an input error (64), not a verdict: a receipt with another `receipt_version` or another `algorithm`, with a field missing, or with a value in another form than the format gives.

## The receipt format, version 1

```
envelope  = { "receipt": <receipt>, "algorithm": "ed25519", "signature": "<base64url, no padding>" }
signature = Ed25519 over the UTF-8 bytes of the RFC 8785 (JCS) canonical JSON of <receipt>
receipt   = {
  "receipt_version": 1,
  "issuer": "<the lab's name, such as a domain>",
  "key_id": "<the first 16 hex characters of the SHA-256 of the raw 32-byte public key>",
  "issued_at": "<RFC 3339, UTC, seconds, ending in Z>",
  "check_id": "<the lab's id for the run, a string>",
  "build_id": "<64 lower-case hex>",
  "result_sha256": "<64 lower-case hex: SHA-256 of the RFC 8785 canonical JSON of the whole result file>",
  "result_hash": "<the result's own deterministic hash, as the result states it, or null>",
  "dyno_version": "<string>",
  "lab": { "kind": "<string>", "backend": "<string or null>" },
  "device": { "key": "<string>", "model": "<string or null>", "os_version": "<string or null>" },
  "protocol": <object: host profile name and version, scenario, spins, device runs>,
  "measures": [ { "id": "<measure id>", "value": <number or null>, "unit": "<string or null>" }, ... ]
}
```

A lab's public keys are a JSON document:

```
{ "keys": [ { "key_id", "algorithm": "ed25519", "public_key": "<base64url of the raw 32 bytes>", "created_at", "retired_at": <time or null> } ] }
```

The rules of a verifier:

- It refuses a `receipt_version` that it does not know. The version is a number, so `1`, `1.0` and `1e0` are all version 1, and all are `1` in the signed bytes.
- It refuses a file in which an object has one name twice, at any depth, also inside a field that it does not know (RFC 8785, section 3.1). Parsers differ in which of the two values they keep, so such a receipt could verify with one value while another reader sees the other. The same holds for the result file and the keys document.
- It ignores a field inside `receipt` that it does not know. The signature still covers that field, because the signature is over the canonical JSON of the whole `receipt` object as the file holds it. So a verifier must not build the signed bytes from only the fields it knows.
- It tries only a key whose key id is the receipt's `key_id`, and only a key that it was given. It works out a key's id from the public key itself.
- It ignores a field outside `receipt`, beside `algorithm` and `signature`. Such a field is not signed.

The forms that dyno holds a version 1 receipt to:

- `signature` is 64 bytes in base64url (RFC 4648, section 5) with no padding: 86 characters.
- `key_id` is 16 lower-case hex characters. `build_id` and `result_sha256` are 64.
- `issued_at` is exactly `YYYY-MM-DDTHH:MM:SSZ`, such as `2026-11-25T10:00:00Z`: no fraction of a second and no offset. It is signed as text, so it has one spelling.
- A public key is its raw 32 bytes (RFC 8032), in base64url with no padding: 43 characters.
- A time in a keys document is RFC 3339, with any offset and any fraction of a second.

JSON Schema for both documents is in `packages/schema/json/`: `receipt.schema.json` and `receipt-keys.schema.json`. The TypeScript types are `ReceiptEnvelope`, `Receipt` and `PublicKeys`. The schema takes only version 1 and `ed25519`, gives the forms above as patterns, and requires every field: a field with no value is null, not left out. dyno itself reads a missing field that can be null as null.

### Canonical JSON

The signature and `result_sha256` are over RFC 8785 canonical JSON (JCS), as UTF-8 bytes:

- No whitespace between tokens.
- The keys of each object sorted by their UTF-16 code units. An emoji is two surrogates, so it sorts before U+FB33. No key is normalized.
- Each number written as ECMAScript writes the double nearest to it: the shortest digits that read back as the same double. So `1.0` is `1`, `-0` is `0`, `1e21` is `1e+21`, `0.0000001` is `1e-7`, and an integer above 2^53 is the double it rounds to: `9007199254740993` is `9007199254740992`.
- Each string with only `"`, `\` and the characters below U+0020 escaped: `\b`, `\t`, `\n`, `\f` and `\r` where JSON has them, and `\u00xx` in lower case for the rest. Every other character is itself, in UTF-8: `/`, U+007F, U+2028 and text that is not ASCII.

In JavaScript, these are the bytes of `JSON.stringify` on each value read by `JSON.parse`, with the keys of each object sorted by `Array.prototype.sort`. dyno gives the same bytes, and tests it with the examples of RFC 8785 sections 3.2.2 and 3.2.3 and the numbers of its appendix B (`crates/dyno-schema/tests/jcs.rs`).

Three inputs differ between the two. dyno refuses an object with one name twice, a number too large for a double, such as `1E400`, and a string with half of a surrogate pair, such as `"\ud800"`, as RFC 8785 says. `JSON.parse` takes all three: it keeps the last value of the name, writes the number as `null`, and writes the half pair as an escape. A signer in JavaScript must refuse them too, or it signs bytes that no strict verifier makes. No result that dyno writes holds any of them.

`result_sha256` is over the result file as it is, with every field, so it covers the run part and any field that a newer dyno added. The spelling of the file does not matter: the same JSON with other whitespace or key order has the same hash.

## A lab's public keys

A lab publishes its keys document where the people who verify can get it, such as at an https URL on its own domain. `dyno receipt keygen` writes one with one key.

| Field        | What it holds                                                                        |
| ------------ | ------------------------------------------------------------------------------------ |
| `key_id`     | The first 16 hex characters of the SHA-256 of the raw 32-byte public key.            |
| `algorithm`  | `ed25519`. dyno leaves out a key of any other algorithm, so a lab can add one later. |
| `public_key` | The raw 32 bytes, in base64url with no padding.                                      |
| `created_at` | When the lab made the key.                                                           |
| `retired_at` | When the lab stopped signing with the key. Null while it still signs with it.        |

dyno works out each key's id from its public key. A document with a `key_id` that is not its key's, with a public key that is not 32 bytes, or with a time that is not RFC 3339, is an input error. So is a document with one `key_id` in two entries, because two entries could give one key two retirement times.

To change keys, a lab adds the new key to the document and sets `retired_at` on the old one. Receipts that the old key signed before that time still verify.

Retiring a key is for a key that the lab stops using. It does not help when a private key is stolen, because the receipt's `issued_at` is written by whoever holds the key, and a thief can write an earlier time. A lab takes a stolen key out of its keys document. Then no receipt that the key signed verifies, and the lab signs the results it still stands behind again with a new key.

## Run a lab

Two commands are all a lab needs. They are in the same public binary, so the tests of `dyno verify` sign with the same code that a lab does.

### `dyno receipt keygen` (Phase 3)

```sh
dyno receipt keygen --key lab-key.json --out keys.json
```

| Flag           | What it does                                                                               |
| -------------- | ------------------------------------------------------------------------------------------ |
| `--key <FILE>` | Write the private key to this file. Required. The file must not be there.                  |
| `--out <FILE>` | Write the public keys document to this file instead of stdout. The file must not be there. |

It makes a new Ed25519 key pair from the system's random bytes. It writes the private key file, which only its owner can read on a Mac or on Linux, and the keys document with the new key. It writes over no file: a private key file or a keys document that is there already is a usage error (64), and so is an `--out` that names the private key file by another path or through a link. A key that is written over is lost, and so are the older keys of a keys document.

Keep the private key file secret. Whoever has it can sign as the lab.

The private key file is a JSON Web Key for Ed25519 (RFC 8037), so other tools read it too, such as WebCrypto's `importKey("jwk", ...)`:

```json
{
  "kty": "OKP",
  "crv": "Ed25519",
  "kid": "<the key id>",
  "x": "<the public key>",
  "d": "<the private key>"
}
```

`x` is the raw 32-byte public key and `d` is the 32-byte private key (the seed of RFC 8032), both in base64url with no padding. `kid` is the key id, and a file from another tool can leave it out. dyno refuses a file whose `x` is not the public key of its `d`, or whose `kid` is not that key's id.

### `dyno receipt sign` (Phase 3)

```sh
dyno check ./dist --driver serve --device --runs 3 --out result.json
dyno receipt sign --key lab-key.json --result result.json --issuer lab.example --check-id run-0001 --out receipt.json
```

| Flag                   | What it does                                                                            |
| ---------------------- | --------------------------------------------------------------------------------------- |
| `--key <FILE>`         | The private key file. Required.                                                         |
| `--result <FILE>`      | The result file from `dyno check`. Required.                                            |
| `--issuer <NAME>`      | The lab's name, such as its domain. Required.                                           |
| `--check-id <ID>`      | The lab's id for the run. Required.                                                     |
| `--lab-kind <KIND>`    | What kind of lab it is, in the lab's own words. Default `own_phones`.                   |
| `--lab-backend <NAME>` | What the lab's devices run on, such as the name of a device cloud. Default: null.       |
| `--issued-at <TIME>`   | When the lab signs, in UTC to the second, such as `2026-11-25T10:00:00Z`. Default: now. |
| `--out <FILE>`         | Write the receipt to this file instead of stdout.                                       |

The receipt is canonical JSON with one final newline. Ed25519 signatures have no random part, so the same key, result and flags always give the same receipt, byte for byte. An `--out` that is the private key file or the result file, by any path or link, is a usage error (64).

Each field of the receipt comes from a flag or from the result file:

| Receipt field       | Where it comes from                                                                                                                                                                                                                                                                              |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `receipt_version`   | 1.                                                                                                                                                                                                                                                                                               |
| `issuer`            | `--issuer`.                                                                                                                                                                                                                                                                                      |
| `key_id`            | The key id of the private key file's key.                                                                                                                                                                                                                                                        |
| `issued_at`         | `--issued-at`, or now.                                                                                                                                                                                                                                                                           |
| `check_id`          | `--check-id`.                                                                                                                                                                                                                                                                                    |
| `build_id`          | `deterministic.build.build_id`.                                                                                                                                                                                                                                                                  |
| `result_sha256`     | The SHA-256 of the canonical JSON of the whole result file.                                                                                                                                                                                                                                      |
| `result_hash`       | `result_hash`.                                                                                                                                                                                                                                                                                   |
| `dyno_version`      | `deterministic.versions.tool`.                                                                                                                                                                                                                                                                   |
| `lab.kind`          | `--lab-kind`.                                                                                                                                                                                                                                                                                    |
| `lab.backend`       | `--lab-backend`, or null.                                                                                                                                                                                                                                                                        |
| `device.key`        | `run.environment.device.key`.                                                                                                                                                                                                                                                                    |
| `device.model`      | `run.environment.device.name`.                                                                                                                                                                                                                                                                   |
| `device.os_version` | `run.environment.os.version`, or null.                                                                                                                                                                                                                                                           |
| `protocol`          | An object with four fields: `host_profile` is `deterministic.inputs.host_profile` (its `name` and `version`), `scenario` is `deterministic.inputs.scenario`, `spins` is `deterministic.inputs.spins`, and `device_runs` is `deterministic.inputs.device_runs`, or null when the result has none. |
| `measures`          | One entry for each entry of `run.measures` whose `source` is `device`, in the result's order, which is by id. `id` is its `id` and `unit` is its `unit`. `value` is its `value` when its `status` is `measured`, and null otherwise.                                                             |

A measure that is `missing` or `invalid` gets a null value. An invalid measure can keep the value that failed its check, and a receipt has no status to say so, so the receipt does not state that value as a reading.

Some results get no receipt, and each is a usage error (64):

- A result that was edited: its `result_hash` does not match its `deterministic` part.
- A result with no build id. A receipt is for the files of a build, so a check of a launch URL alone gets none.
- A result that names no device: `run.environment.device.key` is null. A receipt is for a run on a device, such as a check with `--device`.
- A result of another `schema_version`.

## The known-answer receipt

`fixtures/receipts/` holds one receipt with everything that made it. Another implementation of the format, in any language, can test against the same bytes. The values below do not change.

| File                     | What it holds                                                                                                                                                           |
| ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `result.json`            | A result of `dyno check crates/dyno-cli/tests/data/tiny-build --driver serve --device --runs 3`, on the fake phone of dyno's own tests. No real phone gave its numbers. |
| `test-private-key.json`  | The private key file of the test key.                                                                                                                                   |
| `keys.json`              | The keys document with the test key.                                                                                                                                    |
| `receipt.json`           | The receipt, as `dyno receipt sign` writes it.                                                                                                                          |
| `receipt.canonical.json` | The bytes that are signed: the canonical JSON of the receipt object, with no final newline.                                                                             |

The test key is the first test key of RFC 8032, section 7.1. Its private key is in the RFC, so it is a secret to nobody. Never trust it for a real receipt.

| Value                      | In hex                                                             | In base64url                                  |
| -------------------------- | ------------------------------------------------------------------ | --------------------------------------------- |
| The private key (the seed) | `9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60` | `nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A` |
| The public key             | `d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a` | `11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo` |

The key id is `21fe31dfa154a261`.

The receipt was signed with this command:

```sh
dyno receipt sign --key fixtures/receipts/test-private-key.json --result fixtures/receipts/result.json \
  --issuer lab.example --check-id chk_0001 --lab-backend fake-phone --issued-at 2026-11-25T11:00:00Z
```

The receipt object, spread over lines to read:

```json
{
  "receipt_version": 1,
  "issuer": "lab.example",
  "key_id": "21fe31dfa154a261",
  "issued_at": "2026-11-25T11:00:00Z",
  "check_id": "chk_0001",
  "build_id": "54bc36cfde26ce4949a736033d9f3d251b6f2ba547725ab7e19779f77be911c5",
  "result_sha256": "c36e9f14f9d5f7124781553595d8a64f379007a651cdafd42531cd3558310845",
  "result_hash": "e81eac2b6c4eb2857f836ad82d7bb34b34f5c2abd3b2e2574710c10bf15f722e",
  "dyno_version": "0.1.3",
  "lab": { "kind": "own_phones", "backend": "fake-phone" },
  "device": {
    "key": "iphone-14-pro-max",
    "model": "iPhone 14 Pro Max",
    "os_version": "26.6.2 (23G90)"
  },
  "protocol": {
    "host_profile": { "name": "reels", "version": 2 },
    "scenario": "alone",
    "spins": null,
    "device_runs": 3
  },
  "measures": [
    { "id": "device.footprint_over_baseline", "value": 146800640, "unit": "bytes_change" },
    { "id": "device.kills", "value": 0, "unit": "count" },
    { "id": "device.phys_footprint", "value": 209715200, "unit": "bytes" },
    { "id": "device.phys_footprint_peak", "value": 209715200, "unit": "bytes" }
  ]
}
```

The bytes that are signed are its canonical JSON: 918 bytes, on one line, with no final newline.

```
{"build_id":"54bc36cfde26ce4949a736033d9f3d251b6f2ba547725ab7e19779f77be911c5","check_id":"chk_0001","device":{"key":"iphone-14-pro-max","model":"iPhone 14 Pro Max","os_version":"26.6.2 (23G90)"},"dyno_version":"0.1.3","issued_at":"2026-11-25T11:00:00Z","issuer":"lab.example","key_id":"21fe31dfa154a261","lab":{"backend":"fake-phone","kind":"own_phones"},"measures":[{"id":"device.footprint_over_baseline","unit":"bytes_change","value":146800640},{"id":"device.kills","unit":"count","value":0},{"id":"device.phys_footprint","unit":"bytes","value":209715200},{"id":"device.phys_footprint_peak","unit":"bytes","value":209715200}],"protocol":{"device_runs":3,"host_profile":{"name":"reels","version":2},"scenario":"alone","spins":null},"receipt_version":1,"result_hash":"e81eac2b6c4eb2857f836ad82d7bb34b34f5c2abd3b2e2574710c10bf15f722e","result_sha256":"c36e9f14f9d5f7124781553595d8a64f379007a651cdafd42531cd3558310845"}
```

| Value                                           |                                                                                          |
| ----------------------------------------------- | ---------------------------------------------------------------------------------------- |
| The SHA-256 of the signed bytes                 | `7e6d6b3e28a1912cd0e074400131973ab7e9811cf90350c35a44ed3ecb1f2f7b`                       |
| The signature, in base64url                     | `dZBJ_Y9Q11GLzd5I37aQNKNaArZ0ofkWO2XV50x_oJZroEqZdX9xp8Xbb5oZYM38ygmzkkwoLXOjWqj0GlCuDQ` |
| `result_sha256`, of the canonical `result.json` | `c36e9f14f9d5f7124781553595d8a64f379007a651cdafd42531cd3558310845`                       |
| The build id of `tiny-build`                    | `54bc36cfde26ce4949a736033d9f3d251b6f2ba547725ab7e19779f77be911c5`                       |

An implementation is right when it gets the same `result_sha256` from `result.json`, the same signed bytes from the receipt object, and the same signature from those bytes and the test key. A signature that differs with the same bytes means the key was read wrongly. This command verifies the receipt with everything:

```sh
dyno verify fixtures/receipts/receipt.json --keys fixtures/receipts/keys.json \
  --result fixtures/receipts/result.json --build crates/dyno-cli/tests/data/tiny-build
```

`crates/dyno-receipt/tests/known_answer.rs` checks each value above, and that signing `result.json` again gives `receipt.json` byte for byte.

## Where the code is

| Part                                                  | Where                                                           |
| ----------------------------------------------------- | --------------------------------------------------------------- |
| The types of a receipt, a keys document and a verdict | `crates/dyno-schema/src/receipt.rs`                             |
| Canonical JSON                                        | `jcs` and `parse_json` in `crates/dyno-schema/src/canonical.rs` |
| Keys, signing and verifying                           | `crates/dyno-receipt`                                           |
| The commands                                          | `verify.rs` and `receipt.rs` in `crates/dyno-cli/src`           |

Ed25519 is from `ring`, the library that already does dyno's TLS, so the binary has one crypto library. `dyno mcp` has no receipt tool yet.
