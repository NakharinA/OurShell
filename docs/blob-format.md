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

A `crypto_box` sealed box (X25519 + XSalsa20-Poly1305), prefixed with a version byte:

```
0   1     version = 0x01
1   32    ephemeral X25519 public key
33  n+16  ciphertext
```

## Item envelope

The unit stored locally and synced to the server.

| Field | Type | Encrypted | Notes |
| --- | --- | --- | --- |
| `id` | UUID v4 | no | |
| `type` | string (`host`, `identity`, `key`, `snippet`, `group`, `audit`) | no | |
| `version` | u64 | no | Incremented on every change |
| `org_id` | UUID | no | Always present, even for personal vaults |
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
| Vault key | Sealed to the account X25519 public key | n/a (sealed box) |
| Vault key (recovery copy) | Recovery key | `"ourshell:recovery"` |
| Item key | Vault key | see above |

Argon2id defaults: m = 64 MiB, t = 3, p = 1, 16-byte salt, 32-byte output. The parameters are stored next to the salt so they can be raised later.
