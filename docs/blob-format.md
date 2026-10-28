# Encrypted blob format (v1)

All encrypted data uses one byte-level container. Multi-byte integers are big-endian.

## Ciphertext container

```
offset  size  field
0       1     version     = 0x01
1       1     alg         = 0x01 (XChaCha20-Poly1305)
2       24    nonce       random per encryption
26      n+16  ciphertext  includes the 16-byte Poly1305 tag
```

The caller supplies the AEAD associated data (AAD); it is **not** stored in the container. Decoders must reject unknown `version` or `alg` values and inputs shorter than 42 bytes.

## Sealed box (vault key sealed to an account public key)

An anonymous-sender box built like HPKE base mode, from audited primitives only:

1. Generate an ephemeral X25519 key pair; compute `shared = X25519(eph_sk, recipient_pk)` and reject a non-contributory (all-zero) result.
2. `key = HKDF-SHA256(salt = eph_pk ‖ recipient_pk, ikm = shared, info = "ourshell:seal:v1")`, 32 bytes.
3. Encrypt with the ciphertext container above, AAD = `eph_pk`.

```
0   1     version = 0x01
1   32    ephemeral X25519 public key
33  …     ciphertext container (version, alg, nonce, ciphertext+tag)
```

## Item envelope

The unit stored locally and synced to the server.

| Field | Type | Encrypted | Notes |
| --- | --- | --- | --- |
| `id` | UUID v4 | no | |
| `type` | string (`host`, `identity`, `key`, `snippet`, `group`, `audit`) | no | |
| `version` | u64 | no | Incremented on every change |
| `org_id` | UUID | no | Doubles as the item's **vault id** (personal or team) -- see "Multiple vaults" below |
| `updated_at` | RFC 3339 | no | |
| `key_id` | string | no | Which vault key wraps `wrapped_key` |
| `wrapped_key` | container | yes | Item key encrypted with the vault key, AAD = `"ourshell:item-key:" ‖ id` |
| `ciphertext` | container | yes | Item JSON encrypted with the item key, AAD = `id ‖ "|" ‖ type ‖ "|" ‖ version` |
| `signature` | 64 bytes | no | Ed25519 over the canonical encoding below |

### Canonical signing bytes

```
"ourshell:item:v1" ‖ len32(id) ‖ id ‖ len32(type) ‖ type ‖ u64(version)
‖ len32(org_id) ‖ org_id ‖ len32(key_id) ‖ key_id
‖ len32(wrapped_key) ‖ wrapped_key ‖ len32(ciphertext) ‖ ciphertext
```

`len32` is a 4-byte big-endian length. Binding the version into both the AAD and the signature stops a server from replaying an old ciphertext under a new version.

## Key wrapping

| Wrapped | Wrapping key | AAD |
| --- | --- | --- |
| Account X25519 + Ed25519 secret keys | Master key (Argon2id) | `"ourshell:account-keys"` |
| Account X25519 + Ed25519 secret keys (recovery copy) | Recovery key | `"ourshell:recovery-account"` |
| Vault key | Sealed to a member's account X25519 public key | n/a (sealed box) |
| Vault key (recovery copy, personal vault only) | Recovery key | `"ourshell:recovery"` |
| Item key | Vault key | see above |

Argon2id defaults: m = 64 MiB, t = 3, p = 1, 16-byte salt, 32-byte output. The parameters are stored next to the salt so they can be raised later.

The account keys' recovery copy (`recovery-account`) is new since the multi-vault/team-sharing work below: it lets `Vault::recover` restore the *same* account keypair from the 24-word phrase, instead of generating a new one. That matters because the account's X25519 secret key is also what opens every *other* vault key (see below) -- regenerating it would silently lock the vault out of every team vault it had joined, on top of the already-documented problem of a sync server (and every other device) still verifying signatures against the old key. A vault created before this field existed has no recovery copy of its account keys and falls back to the old regenerate-and-resign behaviour; see `docs/self-hosting.md`.

## Multiple vaults

A device can hold a key for more than one vault at once: its own personal vault, plus any team vault it's a member of. Locally, `Vault` keeps a `vaults` table (`vault_id`, `key_id`, `name`, `kind`, the vault key sealed to this account's own X25519 public key, and this account's `role`) and opens every row in it at unlock time. Each item's `org_id` field says which vault it belongs to, and `key_id` says which generation of that vault's key was used -- the same `org_id`/`key_id` pair the server's `vault_id`/push-pull API already uses, so no wire format changed for this.

**Verifying a shared item's signature.** A team vault's membership changes over time, and an item should stay verifiable as long as it was signed by whoever was a member *when they signed it* -- not just by one fixed "owner" key. Rather than add an explicit `signer` field to the envelope (which would need its own trust bootstrapping: a signer field is only as trustworthy as whatever says it's allowed to sign, which just pushes the problem to a list of keys anyway), each vault's client tracks the *set* of its current members' Ed25519 verifying keys (fetched from the server, `GET /v1/vaults/{id}/members`) and accepts a signature that matches *any* key in that vault's set at verification time. This is simpler (no new wire field, no bootstrapping question of "who's allowed to add a signer"), and it's the same trust boundary the server already enforces for writes (only a current vault member's push is ever accepted) -- the client's signature check is defense in depth on top of that, not a separate authorization layer.

**Key rotation (removing a team member).** `version` is bound into both the AAD of the body ciphertext (via `ciphertext`'s ADD, above) and the signature, specifically so a server can't silently replay an old ciphertext under a bumped version number. That same binding means rotating a vault's key -- generating a fresh key and re-sealing it to every *remaining* member -- can't be done as a cheap "re-wrap the item key, keep everything else" operation if the item's version also needs to advance (which it must, for the normal optimistic-concurrency push to accept the update): the body ciphertext would still be encrypted under the *old* version's AAD. Rotation therefore fully reseals every item in the vault (decrypt under the old key, re-encrypt under the new one, new version, fresh signature) rather than reusing `ItemEnvelope::rewrap`. `rewrap` keeps its narrower job: moving an item into a *different* vault without changing its version (used when sharing a personal item into a team -- see `Vault::share_into_vault`, which gives the moved copy a fresh id rather than reusing the old one, since both AADs above are keyed by `id` too and the local vault stores one row per id across *all* vaults).
