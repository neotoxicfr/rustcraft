# Security

## Reporting a vulnerability

Do not open a public issue. Use GitHub's private reporting: the **Security** tab, then
**Report a vulnerability**. Describe the flaw, its impact and how to reproduce it. You will get an
answer as soon as possible, and the fix will be published with your agreement on whether your name is
mentioned.

Only the latest version of the `main` branch is supported.

## What Rustcraft protects first

- **The session token** passed on by the official launcher: kept in memory only, never written to a
  log or to disk, sent only to Mojang's session services, over verified TLS.
- **Data coming from servers**: packets, NBT, text components, resource packs. A malicious server
  must not be able to crash the client, exhaust its memory or write outside the intended folders.
- **Servers' resource packs**: downloaded only after the player agrees, size-limited, SHA-1
  fingerprint checked.

A flaw in any of these areas is handled first.
