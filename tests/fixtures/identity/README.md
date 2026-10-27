# Public test signing keys

`key-a.der` and `key-b.der` are deliberately committed RSA private keys for synthetic identity-provider tests in `tests/identity.rs`. Their matching `.json` files supply the public JWKs. Treat these keys as public: anyone with this repository can sign with them.

The tests use an owned loopback identity server, a fixture issuer and short-lived synthetic claims. They cover signing-key rotation, identity validation and access boundaries. These files are not deployment credentials and must never be installed in a live identity provider or trusted by a production issuer.

Preserve this context when reviewing history or copying fixtures. A filename/digest appearing in qualification evidence is distinct from the key bytes themselves; binary key material still needs explicit publication review.
