# Upstream provenance

- Crate: `rama-tls-rustls` 0.3.0-alpha.4
- Repository: `plabayo/rama`
- Source commit: `4733273a10a791762e2b7727850032b0c9b8536d`
- crates.io archive SHA-256: `536d47f6b269fb20dffd45e4c04aa8b340698b3509326e3c36e444b4f33ce0d6`
- Upstream license: MIT OR Apache-2.0; this repository distributes the vendored
  source under the root Apache-2.0 license.

This Cosmopolitan-only fork changes the TLS crypto dependencies from AWS-LC to
`ring`. AWS-LC's `urandom.c` requires Linux `RNDGETENTCNT`; Cosmopolitan's
headers do not define it, and AWS-LC aborts if the runtime entropy-count ioctl
fails. Defining the missing ioctl constant would only hide the compile error
and would not make that runtime check valid. The native build continues to use
the published crates.io package; the APE build selects this copy through its
explicit Cargo configuration.
