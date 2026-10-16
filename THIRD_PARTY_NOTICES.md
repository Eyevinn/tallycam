# Third-party notices

TallyCam uses the third-party code below. None of it is copied into this
repository; the license texts ship with each package or crate.

## Swift Atomics

<https://github.com/apple/swift-atomics>, Apache License 2.0 with Runtime
Library Exception, Copyright (c) Apple Inc. and the Swift project authors.
Used as a Swift package dependency (the microphone's sample ring), not copied
into this repository. Its license is distributed with the package source.

## Rust crates of the SRT sender (`core/srt`)

The core's SRT caller (`tallycam-srt`, linked into the app through
`core/ffi`) depends on the crates below, used from crates.io and not copied
into this repository. Their license texts ship with the crate sources; a
release build of the app must carry them (for example in an acknowledgements
screen). Regenerate the list with `cargo tree -p tallycam-srt -e normal`.

| Crate | License | Notes |
| --- | --- | --- |
| `srt-protocol` 0.4.4 (<https://github.com/russelltg/srt-rs>, Copyright Russell Greene and contributors) | Apache-2.0 | The SRT protocol engine; no NOTICE file |
| `aes`, `cipher`, `ctr`, `hmac`, `pbkdf2`, `sha-1`, `digest`, `block-buffer`, `crypto-common`, `inout`, `cpufeatures` (RustCrypto) | MIT OR Apache-2.0 | SRT encryption (AES-CTR, key wrap, PBKDF2) |
| `subtle` | BSD-3-Clause | Constant-time helpers (RustCrypto) |
| `generic-array`, `typenum` | MIT / MIT OR Apache-2.0 | |
| `rand`, `rand_core`, `rand_chacha`, `getrandom`, `ppv-lite86`, `zerocopy` | MIT OR Apache-2.0 (zerocopy also BSD-2-Clause) | |
| `bytes`, `bitflags`, `hex`, `log`, `libc`, `cfg-if`, `num-traits`, `array-init`, `arraydeque`, `keyed_priority_queue`, `take-until`, `streaming-stats`, `thiserror` 1 | MIT, MIT OR Apache-2.0, or Unlicense/MIT | |
| `regex`, `regex-automata`, `regex-syntax`, `aho-corasick`, `memchr` | MIT OR Apache-2.0 (aho-corasick, memchr: Unlicense OR MIT) | Pulled in by srt-protocol (stream id and URI parsing) |
| `url`, `idna`, `form_urlencoded`, `percent-encoding`, `idna_adapter`, `utf8_iter` | MIT OR Apache-2.0 | Pulled in by srt-protocol |
| `icu_*`, `zerovec`, `zerotrie`, `zerofrom`, `yoke`, `tinystr`, `litemap`, `writeable`, `potential_utf` | Unicode-3.0 | Through `idna` |
| `smallvec`, `stable_deref_trait`, `displaydoc`, `indexmap`, `hashbrown`, `equivalent` | MIT OR Apache-2.0 | |

Build-time only (proc macros, not in the binary): `derive_more`,
`convert_case`, `thiserror-impl`, `synstructure`, `syn`, `quote`,
`proc-macro2`, `unicode-ident`, `yoke-derive`, `zerofrom-derive`,
`zerovec-derive`, `zerocopy-derive`.

## Rust crates of the device keys (`core/src/device_key.rs`)

The core verifies device key signatures (in the hub and in Strom) and builds
the signed handshake (in the app) with the RustCrypto crates below, used from
crates.io and not copied into this repository. They are linked into the app
through `core/ffi` too, so a release build must carry their license texts
like those of the SRT crates. Regenerate the list with
`cargo tree -p tallycam-core -e normal`.

| Crate | License | Notes |
| --- | --- | --- |
| `p256` 0.13, `ecdsa`, `elliptic-curve`, `primeorder`, `crypto-bigint`, `sec1`, `der`, `const-oid`, `base16ct`, `rfc6979`, `signature`, `zeroize` (RustCrypto, <https://github.com/RustCrypto>) | MIT OR Apache-2.0 | P-256 ECDSA (verification; signing only for test devices) |
| `ff`, `group` (zkcrypto) | MIT OR Apache-2.0 | Field and group traits under `elliptic-curve` |
| `sha2`, `digest`, `hmac`, `block-buffer`, `crypto-common`, `generic-array`, `typenum`, `cpufeatures` | MIT OR Apache-2.0 | SHA-256 for fingerprints and ECDSA |
| `subtle` | BSD-3-Clause | Constant-time helpers |
| `rand_core` 0.6 | MIT OR Apache-2.0 | Traits only; the core draws no randomness |
| `base64` 0.22 | MIT OR Apache-2.0 | Keys and signatures on the wire |
