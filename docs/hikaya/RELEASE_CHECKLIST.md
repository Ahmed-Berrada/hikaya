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
- [ ] With **Whisper Large v3 Turbo** on the GPU: a sentence comes out as words, not a run of
      random tokens in several alphabets. On an Intel iGPU with the Mesa Vulkan driver it does
      (2026-11-28, with transcribe-cpp 0.4.2 and 0.5.0; the CPU is fine). If the NVIDIA GPU does it
      too, switch acceleration to CPU and open an issue.
- [ ] Voice commands: "Hi Sarah, new line, thanks" types two lines; "I'll call you Monday. Delete
      that. I'll call you Tuesday." types only the last sentence; "À la ligne" and "supprime ça" do
      the same in French. "I added a new line of code" stays as words.

## Learning

- [ ] History → pencil: fix a wrong word and save. A line says what was noted.
- [ ] Make the same fix on a second dictation. Learning shows it under "Suggested rules".
- [ ] Accept it. Say the word again: it now comes out right.
- [ ] Add a rule by hand, dictate the pattern, see the replacement. Switch the rule off, dictate again, see the original.

## Meetings

- [ ] Meetings → Start recording. Play a YouTube talk through the speakers and speak into the
      microphone (headphones off). "Other people: …" shows the speakers, not "Only your microphone".
- [ ] After about 30 s, lines appear under **Others** (the talk) and **You** (your voice), each in
      its language.
- [ ] Stop. The meeting turns "Done" within a minute; its audio is in the app data folder under
      `meetings/<id>/`.
- [ ] Summarize (needs Ollama): TL;DR, decisions and action items in the meeting's language.
- [ ] Save as Markdown and as SRT; open both files.
- [ ] Dictate while a meeting records: dictation still works, and the meeting goes on.

## Recording on the iPhone (only with the hub)

- [ ] On the iPhone, open the hub page → Record. Allow the microphone, record a minute of talk in
      French and English, Stop. "Everything is uploaded." then "Waiting for your computer".
- [ ] On the laptop, with Hikaya running and connected: within five minutes (or after Sync now) the
      recording shows "Transcribed" on the phone, and the meeting is in Meetings on both, with
      **Room** lines and a summary if Ollama runs.
- [ ] Lock the iPhone screen during a recording, unlock it: the recording shows "Interrupted" or
      continues; "Send what was saved" turns what was uploaded into a meeting.

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
