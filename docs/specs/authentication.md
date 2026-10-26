# Online login: account, session, chat keys (26.3)

Sources: reading of the decompiled official client and server and of the authentication
library shipped with the game (kept outside the repository, reworded); community wiki
("Microsoft authentication", "Java Edition protocol/Encryption"); Microsoft documentation
(device code flow, authorization code flow). Links in "References".

Related specs: `connection.md` (handshake, offline login), `transport.md` (frames,
compression, AES-128 CFB8), `chat.md` (chat packets, signatures, last seen),
`players.md` (chat session in the player list, `textures` property).

Important reminder: **the official game does not know the Microsoft account.** The launcher
does the whole Microsoft → Xbox → Minecraft chain and passes the game, as arguments, the
username, the profile UUID, the **Minecraft access token**, and optionally the Xbox identifier
(`xuid`) and the application identifier (`clientId`, telemetry). Rustcraft, which replaces the
game, must either reproduce the "launcher" part (§ 1) or receive these same values from a
third-party launcher (decision, § 7).

---

## 1. Microsoft account → Minecraft token

Five-step chain. All requests over HTTPS, JSON as `application/json` (except the Microsoft
endpoints, which use `application/x-www-form-urlencoded`).

```
Microsoft (OAuth 2.0)  →  MS access token (+ refresh token)
Xbox Live user         →  XBL token + user hash (uhs)
XSTS                   →  XSTS token ("Minecraft" relying party)
Minecraft services     →  Minecraft access token (Bearer, 24 h)
Ownership + profile    →  UUID, name, skins, capes
```

### 1.1 Microsoft OAuth 2.0 ("public" application, no secret)

Tenant: **`consumers`** (personal accounts). `common` or a company tenant give errors;
work/school accounts are not supported.
Scopes: **`XboxLive.signin offline_access`** (`offline_access` is required to obtain a refresh
token).

**Variant A — device code** (no local server, the user types a code):
1. `POST https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode`
   form fields `client_id`, `scope`.
   Response: `device_code`, `user_code`, `verification_uri`, `expires_in` (15 min by
   default), `interval` (s), `message` (localizable with `?mkt=fr-FR`). No
   `verification_uri_complete`.
2. Show `user_code` and `verification_uri` ("copy" button + opening the browser).
3. Poll `POST …/consumers/oauth2/v2.0/token` with `grant_type=urn:ietf:params:oauth:grant-type:device_code`,
   `client_id`, `device_code`, at most once per `interval`.
   Expected errors: `authorization_pending` (retry), `slow_down` (increase the interval,
   RFC 8628 standard), `authorization_declined` (stop), `expired_token` (stop, offer to start
   over), `bad_verification_code` (bug).

**Variant B — system browser + PKCE + local redirect** (smoother):
1. Generate `code_verifier` (43–128 random characters), `code_challenge` =
   base64url(SHA-256(verifier)) without padding, a random `state`.
2. Open in the default browser
   `https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?client_id=…&response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A<port>&response_mode=query&scope=XboxLive.signin%20offline_access&state=…&code_challenge=…&code_challenge_method=S256`
   (`prompt=select_account` to switch account).
3. Listen on `127.0.0.1:<port>` (free port chosen by the system; Microsoft accepts any port
   on `http://localhost` for native clients), receive `?code=…&state=…` or
   `?error=…&error_description=…`, **check `state`**, answer with a small "you can close this
   tab" page, close the listener. The code lives for about 1 minute.
4. `POST …/consumers/oauth2/v2.0/token`: `client_id`, `grant_type=authorization_code`, `code`,
   `redirect_uri` (identical), `code_verifier`, `scope`.

Token response (both variants): `token_type` (`Bearer`), `scope`, `expires_in`
(≈ 3,600 s), `access_token`, `refresh_token`. The MS token is opaque: do not decode it.

**Refresh**: `POST …/token` with `grant_type=refresh_token`, `client_id`,
`refresh_token`, `scope`. The response carries a **new** refresh token: replace the old one
immediately (storage, § 4). Lifetime unspecified ("long"); it can be revoked:
`invalid_grant` / `interaction_required` → restart the interactive login.

### 1.2 Xbox Live (user)

`POST https://user.auth.xboxlive.com/user/authenticate`, headers `Content-Type` and `Accept`
`application/json`:

```json
{ "Properties": { "AuthMethod": "RPS", "SiteName": "user.auth.xboxlive.com",
                  "RpsTicket": "d=<MS access token>" },
  "RelyingParty": "http://auth.xboxlive.com", "TokenType": "JWT" }
```

Prefix `d=` for a token issued by the Microsoft identity platform (Azure); the old "live.com"
flow used a different prefix (see Uncertain points).
Response: `IssueInstant`, `NotAfter` (≈ 14 days), `Token` (XBL token),
`DisplayClaims.xui[0].uhs` (user hash).

### 1.3 XSTS

`POST https://xsts.auth.xboxlive.com/xsts/authorize`:

```json
{ "Properties": { "SandboxId": "RETAIL", "UserTokens": ["<XBL token>"] },
  "RelyingParty": "rp://api.minecraftservices.com/", "TokenType": "JWT" }
```

Response: `Token` (XSTS token, ≈ 16 h), `NotAfter`, `DisplayClaims.xui[0].uhs`.
Failure: HTTP 401 with `Identity`, `XErr`, `Message`, `Redirect`. `XErr` values to translate
into a clear message:

| XErr | Meaning | Message to show |
|---|---|---|
| 2148916227 | account banned from Xbox | "This account is banned from Xbox Live." |
| 2148916233 | no Xbox account | "No Xbox profile: create one on xbox.com or minecraft.net, then try again." |
| 2148916235 | Xbox Live unavailable in the country | "Xbox Live is not available in your country." |
| 2148916236 / 2148916237 | age verification required (South Korea) | "Age verification required on xbox.com." |
| 2148916238 | child account (< 18) outside a family | "Minor account: an adult must add it to a Microsoft family." |
| 2148916262 | undocumented, rare | generic message + code |

Note: according to the wiki, the "child account" error occurs with a third-party Azure
application, not with the official launcher's identifier.

### 1.4 Logging in to the Minecraft services

`POST https://api.minecraftservices.com/authentication/login_with_xbox`

```json
{ "identityToken": "XBL3.0 x=<uhs>;<XSTS token>" }
```

Response: `username` (internal identifier, **not** the profile UUID), `roles`, `access_token`
(Minecraft token), `token_type` (`Bearer`), `expires_in` (86,400 s = 24 h).
HTTP 403 here with an Azure application not approved by Mojang (§ 1.8).

### 1.5 Game ownership

`GET https://api.minecraftservices.com/entitlements/mcstore`, header
`Authorization: Bearer <Minecraft token>`.
Response: `items` (list of `{name, signature}`), `signature` (JWT), `keyId`. Java ownership:
presence of `product_minecraft` / `game_minecraft`; subscription: `product_game_pass_pc`,
`product_game_pass_ultimate`. Empty list → "This account does not own Minecraft: Java Edition."
The signature (RS256) can be verified with Mojang's public key, but this is not needed to
play: the profile (§ 1.6) is enough as practical proof.

### 1.6 Profile

`GET https://api.minecraftservices.com/minecraft/profile` (Bearer).
Response: `id` (UUID without dashes), `name`, `skins` (list of `{id, state, url, variant, alias}`,
`variant` `CLASSIC`/`SLIM`, `state` `ACTIVE` for the worn skin), `capes`
(`{id, state, url, alias}`).
Error `{"path":…, "error":"NOT_FOUND", "errorMessage":…}` (HTTP 404) → account without a profile
(recent purchase or Game Pass never launched): "Create your username on minecraft.net or in the
official launcher."

Profile seen by others and by the official game at startup: session/profile by UUID (§ 1.7)
returns `{id, name, properties:[{name:"textures", value, signature}]}`, `value` = base 64 JSON
described in `players.md` § 3. The property is signed (SHA1withRSA) by the services' "profile
property keys"; the official game only accepts a texture URL if it starts with an allowed
domain.

### 1.7 Where the addresses are: the discovery document

The game 26.3's authentication library no longer hard-codes service addresses: at startup it
reads (without a token) a discovery document
`GET https://discovery.minecraftservices.com/minecraft/client`
(`discovery-staging.minecraftservices.com` in pre-production; chosen by a system property). Shape:

```json
{ "environment": "…", "product": "minecraft",
  "discovery": {
    "authentication": { "endpoints": { "getPublicKeys": { "uri": "…" } } },
    "session":        { "endpoints": { "join": {…}, "verify": {…}, "getProfileById": { "uri": "…{profileId}…" } } },
    "player":         { "endpoints": { "getCertificates": {…}, "getAttributes": {…}, "getBlocklist": {…}, "sendReport": {…} } },
    "profiles":       { "endpoints": { "getByName": {…}, "getManyByName": {…}, "getTexture": { "uri": "…", "validUris": ["…{textureId}"] } } },
    "telemetry":      { "endpoints": { "sendEvents": {…} } } } }
```

Each entry: `uri` and, optionally, `validUris` (allow list, used for texture domains).
Read once at startup, on a separate thread; until the first attempt is finished, callers wait
for it. Failure → new attempt after 1, 2, 4, 8, 16, 32 then 64 min (cap); success → re-read
every 24 h (a failed re-read keeps the previous document). Without a document (or with a
document where `environment = offline`), **no fallback address**: every service call fails as
"services unavailable" (`join` → "authentication servers down"). Rustcraft does the same
(`rc-client/src/session.rs`).
Corresponding historical addresses, documented by the wiki (actual discovery values **not
verified**, no service was contacted for this spec):

| key | historical address |
|---|---|
| session/join | `POST https://sessionserver.mojang.com/session/minecraft/join` |
| session/verify | `GET https://sessionserver.mojang.com/session/minecraft/hasJoined` |
| session/getProfileById | `GET https://sessionserver.mojang.com/session/minecraft/profile/{profileId}?unsigned=false` |
| player/getCertificates | `POST https://api.minecraftservices.com/player/certificates` |
| player/getAttributes | `GET https://api.minecraftservices.com/player/attributes` |
| authentication/getPublicKeys | `GET https://api.minecraftservices.com/publickeys` |

Official HTTP client: connect timeout 5 s, read timeout 5 s; `Authorization: Bearer` for the
player services; common error body `{path, error, errorMessage, details}`.

**Player attributes** (`player/getAttributes`, Bearer): `privileges.onlineChat`,
`multiplayerServer`, `multiplayerRealms`, `telemetry`, `optionalTelemetry` (each
`{enabled}`), `chatPreferences.textCommunication`, `profanityFilterPreferences`,
`banStatus.bannedScopes` (scope `MULTIPLAYER`: `banId`, `expires`, `reason`,
`reasonMessage`). The official game uses them to grey out multiplayer ("multiplayer
disabled by the Microsoft account settings") and to disable chat
(`chat.disabled.profile`). Read failure → "offline" properties (everything allowed on the
interface side; the session server will refuse anyway if the privilege is missing).

### 1.8 Application identifier (`client_id`) — user decision

- **Official launcher**: uses its own Microsoft application identifiers (one for the
  Windows/Microsoft Store launcher, historically a "live.com" flow with a dedicated Xbox
  scope). These applications are approved by default for the Minecraft API and do not trigger
  the "child account" error. Their values circulate publicly but **do not belong to us**:
  using them amounts to presenting ourselves as the official launcher (Microsoft/Mojang terms
  of use, risk of being blocked without notice). This spec does not reproduce them.
- **Third-party application today**:
  1. Register an application in Microsoft Entra (Azure portal, "App registrations"):
     personal Microsoft accounts only; platform "Mobile and desktop applications" with
     redirect `http://localhost`; enable "Allow public client flows" (required for the device
     code); **no secret**.
  2. Request access to the Minecraft API through the Mojang/Microsoft review form
     (short link `aka.ms/mce-reviewappid`, cited by the wiki and by the
     minecraft-launcher-lib library). Without approval, `login_with_xbox` and the other
     services answer 403. Approval delay and criteria unknown.
  3. The identifier obtained may appear in the binary (public client, it is not a secret).
- **Without built-in login**: accept the values from a third-party launcher (arguments
  `--username --uuid --accessToken [--xuid] [--clientId]` like the official game, or an
  input file), Rustcraft then handling neither Microsoft nor refreshing.

Options presented without a choice in § 7.

### 1.9 Lifecycle and refresh

| token | lifetime | renewal |
|---|---|---|
| MS access | ≈ 1 h | by refresh token |
| MS refresh | long, unspecified, rotating | replaced at each use; lost → interactive login |
| XBL | ≈ 14 d | redo 1.2 from a fresh MS token |
| XSTS | ≈ 16 h | redo 1.3 |
| Minecraft | 24 h | redo 1.2 → 1.4 |
| chat keys | § 3.3 | `player/certificates` |

Recommended strategy: at startup, and before each connection to an online server if the
Minecraft token expires in less than 1 h (margin): MS refresh → XBL → XSTS → Minecraft →
profile (the profile can change: username, skin). Do all of this off the render thread, with a
visible state ("Connecting to the account…", "Offline: retry"). Never block singleplayer/offline
play on a network failure.

Errors to tell apart on screen: network/unavailable (retry), XErr (§ 1.3), 403 on the
Minecraft services (application not approved — a configuration bug, not the player's fault),
no game (§ 1.5), no profile (§ 1.6), refresh refused ("Sign in again").

### 1.10 Read-only account services, headers, proxy (done on 18/10/2026)

Survey of game 26.3 and its authentication library 10.0.77 (decompiled outside the repository).

- **Headers**: the services client uses Java's HTTP stack without setting a `User-Agent`:
  `User-Agent: Java/<Java version>` (environment provided by the launcher for 26.3:
  `25.0.1`), `Accept: text/html, image/gif, image/jpeg, */*; q=0.2`, no `Accept-Encoding`.
  POST bodies: `Content-Type: application/json; charset=utf-8`. Any code < 400 is a
  success. Timeouts: connect 5 s, read 5 s. Skin download: same stack, no timeout.
  Server packs: `User-Agent: Minecraft Java/26.3` set explicitly (§ packs).
- **Launcher proxy** (`--proxyHost`, `--proxyPort` = 8080 by default, `--proxyUser`,
  `--proxyPass`): SOCKS, used by discovery, the services, skins and server packs (not by the
  connection to the game server). Unreadable port → no proxy; credentials only if both are
  non-empty; the target is resolved locally. Never the proxies from environment variables.
- **Account profile** (at startup, no token): `session/getProfileById` with
  `unsigned=false` → properties (signed `textures`) and `profileActions` (`FORCED_NAME_CHANGE`,
  `USING_BANNED_SKIN`). It serves as the singleplayer profile when the requested username is
  the account's own (case-insensitive): the player has their skin there.
- **Account properties** (at startup, Bearer): `player/getAttributes` → chat, servers, Realms,
  telemetry, filter, friends flags; chat preferences `ENABLED` / `FRIENDS_ONLY`
  (chat allowed); bans by scope. Failure → offline values: chat, Realms and
  servers allowed, no bans.
- **Multiplayer allowed** = no `--disableMultiplayer` + servers allowed + no `MULTIPLAYER`
  ban + no username to change (based on the profile already read). Otherwise: the
  Multiplayer and Realms buttons on the title screen are greyed out, tooltip
  `title.multiplayer.disabled`, `.banned.name`, `.banned.temporary` (end date present) or
  `.banned.permanent`; the disconnect screen offers "Back to title screen" instead of the
  server list.
- **Chat** (multiplayer server only): `--disableChat` → restriction "disabled by the
  launcher"; chat not allowed by the account → "disabled by the profile" (action: account
  settings, `https://aka.ms/JavaAccountSettings`). Both cut the sending and receiving of
  player messages.
- **Blocklist** (`player/getBlocklist`, Bearer, `blockedProfiles`): re-read on each
  connection to a server if absent or if 120 s have passed since the last request;
  consulted online only (never the nil UUID). A blocked player is treated as hidden
  (messages hidden) and appears in the "Blocked" tab of the social interactions (online
  players), state "Blocked" / "Blocked, offline".
- **Chat keys on disk**: the game (outside the development environment) never writes
  `profilekeys/<uuid>.json` and deletes it on each key request; Rustcraft does the same.
- **Textures**: an address is accepted if it starts with a `validUris` entry of
  `profiles/getTexture` (without `{textureId}`); a single refused address (skin, cape, elytra)
  and the whole property is ignored; without a discovery document, nothing is accepted.
- Still to do: ban notice screens at startup (ban, username to change, refused
  skin); friends, presence, reporting (sending: outside the "read-only" scope).

---

## 2. Connecting to an online server

Continuation of `connection.md` § "Handshake and login". Differences from offline:

1. Client `hello`: username (≤ 16) and **profile UUID** (the one from § 1.6).
2. The `online-mode=true` server answers with the **encryption request** (server `hello`):
   - server identifier: string ≤ 20, **empty** in vanilla since 1.7;
   - public key: byte array (VarInt + bytes), 1,024-bit RSA encoded as X.509
     "SubjectPublicKeyInfo" DER;
   - verification token (challenge): byte array, 4 random bytes in vanilla;
   - "should authenticate": boolean (true if the server verifies the session; false for a
     server that encrypts without verifying, the case of in-memory connections / proxies).
3. The client:
   - draws a 16-byte **shared secret** (cryptographic generator);
   - computes the **"Minecraft-style" digest**: SHA-1 of (server identifier in
     ISO-8859-1) ‖ secret ‖ (public key DER as received); the 20 bytes are read as a
     **signed two's-complement** big-endian integer and written in lowercase hexadecimal
     without leading zeros, preceded by `-` if negative. Wiki vectors (SHA-1 of the text
     alone): `Notch` → `4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48`, `jeb_` →
     `-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1`, `simon` →
     `88e16a1019277b15d58faf0541e11910eb756f6`;
   - if "should authenticate": calls **session/join** (off the network thread):
     `POST https://sessionserver.mojang.com/session/minecraft/join`,
     `Content-Type: application/json` (otherwise 415/403), body

     ```json
     { "accessToken": "<Minecraft token>", "selectedProfile": "<UUID without dashes>",
       "serverId": "<digest>" }
     ```

     Success: HTTP 204, empty body. Errors (`error` field of the body):

     | response | message shown ("Login failed: …") |
     |---|---|
     | `ForbiddenOperationException`, or 401 | invalid session ("restart the game and the launcher" → for us: "sign in to your account again") |
     | `InsufficientPrivilegesException` | multiplayer disabled in the Microsoft account settings |
     | `multiplayer.access.banned`, `FORCED_USERNAME_CHANGE` | banned from online play |
     | ≥ 500, timeout, network | authentication servers unreachable |
     | other 4xx | `errorMessage` (or `error`, or "Status: <code>") |

     Failure → the client **cuts the connection** with this message, **except** for a LAN
     server (opened local world): a simple warning and it carries on.
   - sends the **encryption response** (`key`): secret encrypted with RSA (PKCS#1 v1.5,
     128 bytes), then the challenge encrypted the same way (128 bytes), each as a byte array;
   - **enables AES-128 CFB8 in both directions right after sending** this packet (the packet
     itself goes out in the clear): key = IV = secret; continuous state (`transport.md`).
4. The server decrypts, compares the challenge (otherwise protocol error), recomputes the digest
   and calls `GET …/session/minecraft/hasJoined?username=<username>&serverId=<digest>[&ip=<address>]`.
   `ip` is only sent if `prevent-proxy-connections=true`: the services then refuse if
   the address seen by the server differs from the one that did `join` (partial VPN, proxy,
   IPv4/IPv6 dual stack). Response 200 `{id, name, properties}`; only a player's **last**
   `join` is recognized.
   - profile absent → disconnect `multiplayer.disconnect.unverified_username` ("Failed to
     verify username!");
   - services unreachable → `multiplayer.disconnect.authservers_down`;
   - (integrated singleplayer server: offline profile accepted in both cases).
5. The rest is unchanged: compression threshold, end of login (**signed** profile with the
   `textures` property), acknowledgement, configuration.

Frequent online disconnect messages: `unverified_username`, `authservers_down`,
`duplicate_login` (logged in elsewhere), `name_taken`, `expired_public_key`,
`invalid_public_key_signature`, `chat_validation_failed`, `unsigned_chat`,
`out_of_order_chat` (texts in the game's language, translation keys already handled by the
component rendering).

---

## 3. The player's chat keys

### 3.1 Obtaining

`POST https://api.minecraftservices.com/player/certificates`, `Authorization: Bearer <Minecraft
token>`, empty body. Response:

```json
{ "keyPair": { "privateKey": "-----BEGIN RSA PRIVATE KEY-----\n…\n-----END RSA PRIVATE KEY-----\n",
               "publicKey":  "-----BEGIN RSA PUBLIC KEY-----\n…\n-----END RSA PUBLIC KEY-----\n" },
  "publicKeySignature": "…", "publicKeySignatureV2": "<base64>",
  "expiresAt": "<ISO 8601>", "refreshedAfter": "<ISO 8601>" }
```

- Despite the "RSA" PEM header, the base 64 content (MIME, wrapped lines) is a **PKCS#8**
  private key and an **X.509 SubjectPublicKeyInfo** public key, DER (2,048-bit RSA in practice).
- Only `publicKeySignatureV2` is used; the old `publicKeySignature` is ignored.
- Public key or v2 signature missing/empty, unreadable date → no key (the game continues
  without a chat session).
- HTTP failure → no key; the official game then keeps the previous pair if there is one.

Key v2 signature (checked by the server, and by each client for the other players):
SHA1withRSA, by one of the `playerCertificateKeys` keys from `publickeys`, over
profile UUID (16 big-endian bytes) ‖ `expiresAt` in milliseconds (big-endian `i64`) ‖
public key DER. `publickeys` returns `{profilePropertyKeys:[{publicKey}], playerCertificateKeys:[{publicKey}]}`
(DER keys in base 64); a client that does not verify the others can do without it, but then
"enforced secure chat" is treated as not enforced on the client side (§ 3.5).

### 3.2 When the client requests and sends its session

- On receiving the game login packet (`login` of the play phase), the client resets
  its chat session (no session, "unsigned" encoder, index 0, last-seen tracking and
  cache cleared, see `chat.md`), reads the **"online server"** and **"enforced secure
  chat"** flags, and **only if the server is online** starts obtaining the pair (§ 3.1)
  off-thread.
- When the pair arrives (checked at each client tick) and differs from the current pair:
  new session = **random session UUID** + pair; new signature encoder
  (index 0); sending of **`chat_session_update`** (play, server-bound):

  | field | type |
  |---|---|
  | session UUID | UUID (128 bits) |
  | key expiry | `i64`, milliseconds since the Unix epoch |
  | public key | byte array ≤ 512 (X.509 DER) |
  | key signature | byte array ≤ 4,096 (decoded v2 signature) |

  Same structure as action 1 of `player_info_update` (`players.md`): the server
  rebroadcasts it to the other players.
- The official game does not keep the pair on disk (the `profilekeys/<uuid>.json` file is
  only written in a development environment and otherwise deleted).

### 3.3 Expiry and renewal

- `refreshedAfter`: date from which renewal is needed; `expiresAt`: end of validity
  (community order of magnitude: renewal ≈ 40 h, expiry ≈ 48 h — not verified).
- Check at each client tick: if a session exists, **at least 1 h** has elapsed since the last
  attempt and `refreshedAfter` has passed (or there is no pair), request a pair again; if it
  differs → new session (new UUID) and a new `chat_session_update`.
- Server: if the new key expires **before** the old one → disconnect
  `multiplayer.disconnect.expired_public_key`; invalid key signature →
  `invalid_public_key_signature`; if the server does not have the services' public keys, the
  update is ignored.
- Messages signed with an expired key: refused (`chat.disabled.expiredProfileKey`); to
  validate **other** players' messages, an **8 h** grace after `expiresAt` is allowed.
- Clock: a message's timestamp is signed in **seconds**, sent in milliseconds;
  the server warns beyond a 5 min difference, the client hides/reports beyond 7 min.
  A wrong system clock breaks secure chat (`out_of_order_chat`).

### 3.4 Signing messages and commands

Packet format, "last seen", signature cache: `chat.md` § 1–2. Signed content
(SHA256withRSA, session private key, 256-byte signature), in order, big-endian integers:

1. `i32` version = 1;
2. player UUID (16 B); session UUID (16 B); `i32` message index in the session
   (0, 1, 2…; each signed message **or argument** consumes an index);
3. `i64` salt; `i64` timestamp in **seconds**; `i32` UTF-8 length of the content; content;
4. `i32` number of "last seen"; then their full signatures (256 B each), in the
   order of the list sent.

Commands: one signature per **signable argument** ("message"-type arguments of the command
tree), same scheme with content = the argument's text, same timestamp, salt and last seen
for the whole command, index incremented at each argument. Without a signable argument:
plain `chat_command`. Without a session: signatures absent (`chat.md` § 2.1–2.2).

### 3.5 Enforced secure profile (`enforce-secure-profile`) and missing keys

- Server side, enforcement is only effective if `enforce-secure-profile=true` **and**
  `online-mode=true` **and** the server has the services' public keys; it is
  announced in the `login` packet ("enforced secure chat" flag).
- Client side, "enforced" = server flag **and** client able to verify the profile keys
  (`publickeys` keys loaded).
- Enforcing server + player without a chat session: the connection succeeds, but **every
  message** (or command with a signable argument) is refused: `chat.disabled.missingProfileKey`
  ("Chat disabled: profile public key missing"); an unsigned command with a signable argument
  → `chat.disabled.invalid_command_signature`.
- Non-enforcing server: the official game shows a "server not secure" warning once;
  unsigned messages go through.
- If obtaining the keys fails while the server enforces: show the reason in the chat
  ("chat key unavailable, sign in again") rather than sending messages doomed to be refused.

---

## 4. Local storage

### 4.1 What the official launcher keeps (observation, content **not read**)

In `%APPDATA%\.minecraft` (Microsoft Store version): `launcher_accounts_microsoft_store.json`
(list of accounts and profiles), `launcher_msa_credentials_microsoft_store.bin` (encrypted
Microsoft credentials, binary), `launcher_entitlements_microsoft_store.json` (rights/ownership),
`launcher_profiles.json` (installations, no token). The game itself only receives the
Minecraft token as an argument and writes no token. **Never read or reuse these files.**

### 4.2 What Rustcraft must store — *implementation choice*

- To keep: **MS refresh token** (the only durable secret), account identifier
  (profile UUID and name, for display), last login date. Optional: Minecraft token
  and its expiry date (avoids the full chain at each launch; 24 h).
- Not to keep: XBL/XSTS tokens (redone on demand), chat key pair (in memory
  like the official game), device code, `code_verifier`.
- Protection on Windows, in order of preference:
  1. **Windows Credential Manager** ("Credential Manager" vault, target
     `Rustcraft/<uuid>`): encrypted by DPAPI, tied to the user session;
  2. **DPAPI** (`CryptProtectData` user scope, with extra entropy specific to
     Rustcraft) on an `accounts.dat` file in the game folder (`servers::game_dir()`);
  - other systems: the system keyring (Secret Service, macOS Keychain), otherwise refuse to
    remember (login at each launch) rather than write in the clear.
- Accounts file: atomic write (temporary file + rename), permissions restricted to
  the user, immediate replacement of the refresh token at each rotation.
- "Sign out" button: erase the vault entry (Microsoft does not offer simple revocation
  of a refresh token by a public application).

---

## 5. Security

- **Never log** a token (MS, refresh, XBL, XSTS, Minecraft), an authorization/device
  code, a `code_verifier`, a private key, the shared secret. HTTP errors
  are logged without the request body or the `Authorization` header; also mask the local
  redirect URL (it carries the code). The username and UUID may be logged.
- **TLS verified** on every call (certificates and host name, system root store or
  built-in root list); no "ignore certificates" option, no HTTP fallback.
  Only exception: the local redirect `http://localhost:<port>` (loopback).
- **The token is never sent to a game server**: only the digest and the encrypted secret
  go to the server; the token goes only to the Microsoft/Mojang services. A malicious
  server can only get a `join` validated for itself.
- Local redirect: listen only on `127.0.0.1`, random port, a single request, check
  `state`, close after receipt or after a few minutes.
- Shared secret, message salt, `code_verifier`, `state`: the system's cryptographic
  generator.
- Memory: do not keep tokens in displayable structures (F3, crash reports);
  masked `Debug` for the types that carry them.
- Textures: only accept skin/cape URLs on the allowed domains (§ 1.6/1.7) and
  only if the property is signed when that is required.

---

## 6. Uncertain points

- ~~Actual content of the discovery document~~: queried without a token on 12/10/2026
  (`environment` = `prod`); `session/join`, `session/verify`, `player/getCertificates`,
  `authentication/getPublicKeys` do return the historical addresses of § 1.7 (plus
  `loginXbox`, the presence update, the friends list… not used here).
- `RpsTicket` prefix (`d=` for an Azure token; no prefix or `t=` for the old
  live.com flow): depends on the token source; to confirm on the first try.
- Exact lifetimes of the chat keys (`refreshedAfter`, `expiresAt`) and key size (2,048
  bits): community orders of magnitude, to be measured.
- Lifetime and rotation policy of the refresh token for personal accounts:
  unspecified by Microsoft.
- Process and delay for approving Minecraft API access for a new Azure
  application (form not consulted).
- Whether the "child account" error (2148916238) only affects third-party applications:
  wiki claim, not verified.
- `XErr` errors 2148916262 and other undocumented codes.
- Exact behavior of the services if `selectedProfile` is sent with dashes (the game sends
  without dashes; do not deviate).

## 7. Decisions for the user

1. **Application identifier**:
   a) register a "Rustcraft" Azure application and request access to the Minecraft API
      (clean, but unknown delay and possible refusal; "child account" errors possible);
   b) reuse the official launcher's public identifier (works right away, but
      impersonates the official application, against the terms, revocable at any time);
   c) no built-in login: Rustcraft receives username/UUID/token from a third-party launcher
      (arguments like the official game); no durable secret on our side.
2. **Login flow** (if 1a/1b): device code (simple, no network listening), browser
   + PKCE + local redirect (smoother), or both (default browser, device code
   as fallback).
3. **Service addresses**: follow the discovery document like the official game, or
   hard-code the historical addresses (with or without discovery first).
4. **Storage**: Windows credential vault or DPAPI file; also remember the Minecraft token
   (24 h) or only the refresh token; several accounts or just one.
5. **Verifying other players' keys** (`publickeys`, v2 signatures, signatures of
   received messages): from the first online milestone, or later (chat stays "unverified"
   and enforcement is seen as not enforced on the client side).
6. **Account attributes** (multiplayer/chat disabled, bans): read them and grey out the
   interface like the official game, or let the session server refuse.

## References

- https://minecraft.wiki/w/Microsoft_authentication
- https://minecraft.wiki/w/Java_Edition_protocol/Encryption
- https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code
- https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow
- RFC 8628 (device code), RFC 7636 (PKCE)
