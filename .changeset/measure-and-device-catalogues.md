---
default: minor
---

# The schema package lists every measure and every known device

`@dyno-labs/schema` now has `json/measures.json`, every measure id with its source, phase, unit and scope, and what it counts (`quantity`) and which reading it is (`statistic`), and `json/devices.json`, every iPhone from the iPhone XS and XR to the iPhone 17 family, by the key a device run gives its phone. It also has `json/dyno-toml.schema.json`, the shape of `dyno.toml`.
