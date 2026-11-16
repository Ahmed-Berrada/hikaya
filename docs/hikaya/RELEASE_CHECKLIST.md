# Release checklist

Automated tests cover the logic and the screens. They cannot press a real hotkey on Windows, hear a
real microphone there, or type into another app. This list is the manual part. It takes about ten
minutes on the Windows machine. Anything that fails becomes an issue before the release is published.

## Before building

- [ ] `main` is green: Rust tests, `hikaya crates`, code quality, Playwright.
- [ ] `scripts/hikaya/e2e-linux.sh` passes on the Linux machine (real audio through the real app).
- [ ] The version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json` is the one being released.

## Install (Windows)

- [ ] Download the `-setup.exe` from the draft release and run it. SmartScreen warns because the
      installer is not code-signed: **More info → Run anyway**.
- [ ] The app is called Hikaya in the Start menu, the window title and the tray.
- [ ] If Handy is also installed, both start and neither shows the other's history or settings.

## First run

- [ ] Onboarding completes: microphone chosen, English and French picked, a model downloaded.
- [ ] Settings → Advanced: the acceleration setting offers the NVIDIA GPU.

## Dictation

Open Notepad and use the transcribe shortcut.

- [ ] An English sentence is typed where the cursor is.
- [ ] A French sentence is typed, in French.
- [ ] A sentence that starts in French and ends in English keeps both languages.
- [ ] "Euh" and "um" said on purpose do not appear.
- [ ] The text also lands in Slack or a browser field, not only Notepad.
- [ ] Time from releasing the key to seeing text, for about ten seconds of speech: \_\_\_ s.

## Learning

- [ ] History → pencil: fix a wrong word and save. A line says what was noted.
- [ ] Make the same fix on a second dictation. Learning shows it under "Suggested rules".
- [ ] Accept it. Say the word again: it now comes out right.
- [ ] Add a rule by hand, dictate the pattern, see the replacement. Switch the rule off, dictate again, see the original.

## Sync with the hub (only if Tailscale is running on this machine)

- [ ] Sync → address `https://<hub>.<tailnet>.ts.net`, a device name, the hub passphrase → Connect.
      The status turns to "Up to date" and the existing history is sent.
- [ ] Dictate something; within a few seconds "Nothing waiting to be sent" again.
- [ ] Add a rule here; it appears on another connected device after its next sync.
- [ ] Turn Wi-Fi off, dictate: "Hub unreachable" and a waiting count. Turn it back on: sent.
- [ ] After the hub machine reboots: "Hub locked"; unlock from this screen.

## AI cleanup (only if Ollama is installed)

- [ ] Settings → Post Processing: provider Custom (`http://localhost:11434/v1`), a model picked,
      prompt "Hikaya Cleanup (English + French)".
- [ ] Dictate a mixed sentence with the post-processing shortcut: punctuation is fixed and nothing
      is translated. If the model translated, the text must come out as spoken, with a warning.

## Update and uninstall

- [ ] "Check for updates" answers without an error.
- [ ] Uninstall removes the app; reinstalling keeps history and rules.

## Publish

- [ ] Every box above is ticked, or has an issue linked next to it.
- [ ] Publish the draft release.
