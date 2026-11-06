# Vendored Camoufox Juggler driver

This crate is vendored from `Pedrvisk/camoufox-rust` revision
`933ae2e8ba7a00e254b4d4075eb0e85a527289fc` (crate version `0.9.1`).

Code carries two narrow integration extensions that are not available in that
upstream revision:

- `launch_prepared_with_juggler`, which launches one already-prepared Camoufox
  fingerprint/config contract without generating a second randomized identity;
- `JugglerBrowser::attached_targets`, which broadcasts root
  `Browser.attachedToTarget` events so popup-created pages can be adopted without
  replacing the driver's root-session subscription.

Keep the rest of the crate source aligned with the pinned upstream revision.
The upstream license is preserved in `LICENSE.md`.
