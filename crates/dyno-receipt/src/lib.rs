//! A lab's signed receipt for a check result: the keys, the signing and the verifying.
//!
//! A lab runs dyno on real phones for people who do not own them, and signs a receipt for each
//! result. Someone who receives the build then trusts the lab's phone readings for exactly the
//! files they hold: [`verify::verify`] checks the receipt's signature against the lab's public
//! key, and compares the receipt with the result file and with the build id of the files.
//!
//! The format is open, and so is this code, so anyone can run a lab: [`keys::SigningKey`] makes a
//! key pair, and [`sign::receipt_of_result`] and [`sign::sign`] make a receipt for a result. The
//! types are in `dyno_schema::receipt`, and `docs/receipts.md` gives the format.
//!
//! This crate reads no file and opens no connection. `dyno-cli` does both.

mod encoding;
pub mod keys;
pub mod sign;
mod times;
pub mod verify;

pub use times::{is_receipt_time, receipt_time};
