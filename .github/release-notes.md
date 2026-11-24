Klirr {{VERSION}}: the free & open-source guitar amp, as a desktop app. Plug a guitar into your
audio interface and play. This is an early release: please report what goes wrong, from the
app's Help menu or to support@bechtstudio.com.

## Downloads

| System | File |
|---|---|
| Windows 10 and 11 (64-bit) | `Klirr_{{VERSION}}_x64-setup.exe` |
| macOS (Apple silicon and Intel) | `Klirr_{{VERSION}}_universal.dmg` |
| Linux (x86-64) | `Klirr_{{VERSION}}_amd64.AppImage`, or `Klirr_{{VERSION}}_amd64.deb` for Debian and Ubuntu |

**Windows:** the installer offers *ASIO support*, selected by default: the lowest latency with
most audio interfaces. With it, Klirr includes the Steinberg ASIO SDK and is licensed under the
GPLv3 instead of MIT; without it, Klirr uses WASAPI. The installer is not signed yet, so
Windows may say it "protected your PC": choose *More info*, then *Run anyway*.

**macOS:** the app is not notarised yet. On the first start, open it with a right-click and
*Open*, or allow it under *System Settings › Privacy & Security*. Klirr asks for the
microphone: that is the input of your audio interface.

## Checking the downloads

`SHA256SUMS` holds the checksum of every file (`sha256sum -c SHA256SUMS`). Each file also has
a build attestation: proof that this repository's release workflow built it from tag
`{{TAG}}`. With the [GitHub CLI](https://cli.github.com):

```sh
gh attestation verify Klirr_{{VERSION}}_x64-setup.exe --repo MaximBecht/klirr
```

## Source and licenses

Klirr is MIT-licensed; the Windows build with ASIO support is GPLv3, as above. Every app
installs its licenses and the notices of the third-party software it contains.
`klirr-{{VERSION}}-source.tar.gz` is the complete source of this release, with the ASIO SDK the
Windows build was made with.
