# Voice note from an iPhone Shortcut

A Shortcut records a voice note, sends it to your hub, waits for your computer to transcribe it,
and copies the text to the clipboard. It suits short notes (up to about fifteen minutes). For a
meeting, use **Record** in the hub's web page instead ([hub README](../../hub/README.md)).

The transcription happens on your computer, so the Hikaya app must be running and connected to the
hub. It looks for new recordings about every 30 seconds.

## 1. Give the Shortcut its own device token

Each Shortcut is a device, so you can revoke it alone. On a computer on your tailnet, run (replace
the address and type your passphrase when asked):

```bash
HUB=https://<machine>.<tailnet>.ts.net
read -rsp "Hub passphrase: " PASS; echo
curl -s -X POST "$HUB/api/devices" -H 'content-type: application/json' \
  -d "{\"passphrase\":\"$PASS\",\"name\":\"iPhone Shortcut\"}"
```

Copy the `token` from the answer. Keep it like a password: anyone with it can read your archive.

## 2. Build the Shortcut

In the Shortcuts app, create a new Shortcut named "Hikaya note" with these actions:

1. **Record Audio.** Audio quality: Normal. Start recording: Immediately. Finish recording: On Tap.
2. **Get Contents of URL**
   - URL: `https://<machine>.<tailnet>.ts.net/api/recordings/upload?title=Voice%20note`
   - Method: POST
   - Headers: `Authorization` = `Bearer <token>`, `Content-Type` = `audio/mp4`
   - Request Body: File → _Recorded Audio_
3. **Get Dictionary Value** for `id` in _Contents of URL_. Rename the result to _Recording_.
4. **Repeat** 40 times:
   1. **Wait** 15 seconds.
   2. **Get Contents of URL**: `https://<machine>.<tailnet>.ts.net/api/recordings/` + _Recording_ +
      `/text`, Method GET, the same `Authorization` header.
   3. **Get Dictionary Value** for `status`.
   4. **If** _Dictionary Value_ is `done`:
      1. **Get Dictionary Value** for `text` in the answer of step 4.2.
      2. **Copy to Clipboard**.
      3. **Show Notification**: "Copied: " + the text.
      4. **Stop This Shortcut**.
   5. **End If**
5. **Show Notification**: "Your computer will transcribe it on its next sync."

Add it to the Home Screen or the Action button. The note also appears as a meeting, with its
transcript, under Meetings in the app and on the hub's page.

## What the hub answers

`GET /api/recordings/<id>/text` returns:

| `status`  | Meaning                                            | `text`         |
| --------- | -------------------------------------------------- | -------------- |
| `pending` | Waiting for a computer                             | `null`         |
| `claimed` | A computer is transcribing it                      | `null`         |
| `done`    | Transcribed                                        | the transcript |
| `failed`  | Transcription failed three times; `error` says why | `null`         |

To stop the Shortcut's access, revoke its device on the hub.
