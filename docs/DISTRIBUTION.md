# Distributing Kudu Desktop without install warnings

Releases are automatic: every push to `main` whose Linux, macOS and Windows builds all
succeed becomes a GitHub release (`.github/workflows/build.yml`), and the site's
https://kuduagent.vercel.app/download page always offers the latest one.

What the user sees when installing depends only on **signing**:

| OS | Unsigned (today) | How to remove the warning | Cost |
|---|---|---|---|
| Windows | "Windows protected your PC" (SmartScreen) → More info → Run anyway | **Microsoft Store (MSIX)** - Microsoft signs the package, no SmartScreen | Free (individual account) |
| Windows | same | OV code-signing certificate on the direct download - SmartScreen still warns until the certificate earns reputation | ~€49-350 / year |
| macOS | Gatekeeper: "can't be opened" / "is damaged" | Apple Developer ID signing + notarization | $99 / year |
| Linux | none | nothing needed | - |

There is no legitimate way to make an unsigned Windows or macOS app install silently -
the warnings exist precisely to stop that. Signing is the only fix.

## 1. macOS - Developer ID + notarization (recommended, fully automatic)

1. Join the Apple Developer Program (individual, $99/year): https://developer.apple.com/programs/
2. In Xcode or https://developer.apple.com/account/resources/certificates create a
   **Developer ID Application** certificate. Export it from Keychain Access as a `.p12`
   with a password.
3. Create an app-specific password for your Apple ID at https://account.apple.com
   (Sign-In and Security → App-Specific Passwords).
4. Add these **repository secrets** (GitHub → Settings → Secrets and variables → Actions):
   - `APPLE_CERTIFICATE` - `base64 -i cert.p12 | pbcopy` (macOS) or
     `[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.p12"))` (PowerShell)
   - `APPLE_CERTIFICATE_PASSWORD` - the .p12 password
   - `APPLE_SIGNING_IDENTITY` - e.g. `Developer ID Application: Gloire Mbonyi (ABCDE12345)`
   - `APPLE_ID` - your Apple ID email
   - `APPLE_PASSWORD` - the app-specific password
   - `APPLE_TEAM_ID` - the 10-character team id
5. Push to `main`. The next release's `.dmg` is signed and notarized; the release notes
   say so. Nothing else changes.

## 2. Windows - Microsoft Store (recommended: no SmartScreen, free)

Store installs are signed by Microsoft and never show SmartScreen. Individual developer
registration is free since September 2025 (ID + selfie check, ~200 markets).

1. Register: https://developer.microsoft.com/microsoft-store/register (individual).
2. Partner Center → Apps and games → New product → **MSIX or PWA app**, reserve the name
   "Kudu Desktop". Under Product identity copy **Package/Identity/Name**,
   **Package/Identity/Publisher** (`CN=...`) and **Publisher display name**.
3. Package the Windows build as MSIX with Microsoft's winapp CLI (it has a Tauri guide:
   https://learn.microsoft.com/windows/apps/dev-tools/winapp-cli/guides/tauri):
   ```powershell
   winget install microsoft.winappcli --source winget
   # from the repo root, after `npm run tauri build`
   winapp init        # creates Package.appxmanifest + Assets; fill Identity with the step-2 values
   winapp pack        # produces the .msix (the Store re-signs it - no certificate needed)
   ```
4. Upload the `.msix` in the submission, fill the listing (screenshots, privacy policy
   URL `https://kuduagent.vercel.app/privacy`), and submit for certification.
5. When it is live, set `MICROSOFT_STORE_URL` in the site's download page so Windows
   visitors get "Get it from Microsoft" as the first option.
   (Once the manifest is in the repo, a CI job can build the `.msix` on every release and
   the Microsoft Store CLI can submit it - ask for it when the Store account exists.)

## 3. Windows - code-signing certificate on the direct download (optional)

Signing the `.exe`/`.msi` proves who published it, but with a new certificate SmartScreen
keeps warning until enough users have installed the app. Choose this in addition to the
Store, not instead of it.

- Individuals outside the US/Canada cannot use Microsoft Artifact Signing (Azure); buy an
  **OV code-signing certificate** from a CA (Certum, Sectigo, SSL.com, DigiCert…). Since
  2023 these are issued on a hardware token or cloud HSM - pick a product that can export
  for CI or offers a CI signing tool.
- If you have a `.pfx`, add the secrets `WINDOWS_CERTIFICATE` (base64 of the .pfx) and
  `WINDOWS_CERTIFICATE_PASSWORD`; the workflow imports it and Tauri signs both installers.
- A company registered in an eligible country can use Microsoft Artifact Signing instead
  (Tauri `bundle.windows.signCommand`, see https://v2.tauri.app/distribute/sign/windows/).

## 4. Linux

No signing needed. The `.AppImage` runs anywhere (`chmod +x`), `.deb`/`.rpm` install with
the system package manager. Optional later: Flathub for one-click installs in GNOME
Software / KDE Discover.
