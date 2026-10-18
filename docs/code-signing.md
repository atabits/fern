# Code signing

Release packages are built by `.github/workflows/release.yml`. Signing turns on once its
credentials are set in the repository (Settings → Secrets and variables → Actions). Until then
the packages are built unsigned:

- Windows SmartScreen warns about an unknown publisher.
- macOS Gatekeeper refuses to open the app until it is allowed in System Settings.

## Windows: SignPath Foundation

[SignPath Foundation](https://signpath.org) signs open-source projects for free, with a
certificate issued to the foundation.

1. Apply on signpath.org with the repository's URL. Once accepted, SignPath creates an
   organization and a project for Fern.
2. Install the SignPath GitHub app on the repository. It lets SignPath verify that what it
   signs was built by this repository's workflow.
3. In the SignPath project:
   - Name the project `fern`.
   - Keep the signing policies `test-signing` and `release-signing`. Release signing waits for
     a manual approval in SignPath.
   - Add two artifact configurations:
     - `windows-programs`: a zip holding the PE files `fern.exe` and `fern-server.exe`.
     - `windows-installer`: a zip holding one PE file, `*-setup.exe`.
4. In the GitHub repository:
   - Add the variable `SIGNPATH_ORGANIZATION_ID`.
   - Add the secret `SIGNPATH_API_TOKEN`, a CI user's token from SignPath.

The workflow first signs the programs, then builds the installer from them, and finally signs
the installer.

## macOS: Developer ID and notarization

This needs the Apple Developer Program (99 USD a year).

1. Create a "Developer ID Application" certificate and export it with its private key as a
   `.p12` file.
2. Create an app-specific password for your Apple ID at appleid.apple.com.
3. Add these repository secrets:

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | the `.p12` file, base64-encoded (`base64 -i cert.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` file's password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | the Apple ID's email |
| `APPLE_TEAM_ID` | the ten-character team ID |
| `APPLE_APP_PASSWORD` | the app-specific password |

`script/package` then:
- signs `Fern.app` with the hardened runtime;
- signs the DMG;
- sends the DMG to Apple for notarization;
- staples the ticket to the DMG, so it opens without a warning even offline.

## Code signing policy

This policy applies to signed Windows releases.

- Free code signing is provided by [SignPath.io](https://signpath.io), with a certificate by
  [SignPath Foundation](https://signpath.org).
- Committers and reviewers: [Amin Atabiev](https://github.com/atabits).
- Approvers: [Amin Atabiev](https://github.com/atabits).

Only binaries built by this repository's GitHub Actions from its own source are signed.

Fern does not send any data anywhere unless you set up sync with a server of your own choosing.
