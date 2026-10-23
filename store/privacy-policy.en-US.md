# Molt Privacy Policy

Last updated: October 22, 2026

Molt is a disk cleanup tool for macOS. This document explains what data it touches, where that data goes, and how to turn it off.

In one sentence: scanning and verdicts happen entirely on your Mac and are never uploaded. A summary of the current scan is sent to a model provider only when you choose to use AI analysis and configure that provider yourself.

## 1. Processed locally, never uploaded

- **File paths and sizes.** Scanning reads file system metadata (path, size, allocated size, file count, modification time) to build the list and the verdicts.
- **Verdicts.** Every item is classed as reclaimable, costly or protected by deterministic rules built into the binary. No network request is involved.
- **Cleanup history.** Which item was moved, restored or deleted, and when, is recorded in a local audit database (`~/Library/Application Support/app.molt/`).
- **Quarantine contents.** Moved files are kept as they are in a local quarantine folder and cleaned up according to your settings after seven days.

None of this leaves your device. Molt has no accounts, no sign-in, and no analytics, tracking or crash-reporting SDK.

## 2. What is sent out

### AI analysis (off by default, requires your configuration)

This is the only feature that uses the network. With no model provider configured, the app makes no requests at all.

When you click AI analysis and have selected a provider (OpenAI, Anthropic, DeepSeek, or any compatible endpoint including a local Ollama instance), Molt sends **to the address you configured**:

- the **metadata** from this scan: paths relative to your home folder, item type, logical and allocated size, file count, last modification date, verdict category and note, and the rule that matched;
- total and available disk space;
- if the item count exceeded the cap, how many were omitted.

The summary has a hard cap (at most 120 items, the largest by size), so the amount sent does not grow with disk size.

**File contents are not sent by default.** Only if you explicitly enable content sampling does Molt read the first few bytes of small text files that meet the trigger conditions. Before sending, the sample passes a secret-fingerprint check; on a match the whole sample is discarded and only "this looks like a secret" is reported.

**API keys** live in your own configuration file or environment variables. They are used only to call the provider you selected, are never sent anywhere else, and never appear in logs or error messages.

### What the provider does with it

The provider you choose (DeepSeek, OpenAI, Anthropic, or your own Ollama) handles the received summary under its own privacy policy. That part is outside Molt's control. Do not place information you would not share with that provider in folders that will be scanned, or use a model running on your own machine.

### What is never collected

Molt does not collect or upload your name, email address, device identifiers, usage statistics or crash reports. Beyond the connection necessarily made to the model provider you selected, no network traffic is generated.

## 3. macOS permissions

Reading some cache folders under `~/Library` requires Full Disk Access in System Settings → Privacy & Security. That permission is granted by macOS, and Molt can only read paths the system allows. Without it the app still works with a smaller scope and tells you which folders could not be read.

## 4. Your choices

- Configure no model provider: AI analysis stays unavailable, everything else works, and the app does not use the network.
- Remove keys from the configuration, switch to environment variables, or point the provider at a local model.
- Permanently delete quarantined files at any time, or clear the audit records.
- Uninstalling stops all processing; the configuration and audit database left on disk can be deleted directly.

## 5. Children

Molt is a tool for developers and is not directed at children under 13. It does not knowingly collect information from them.

## 6. Changes

Updates to this policy ship with app releases and change the date above.

## 7. Contact

Questions: **support@molt.app**

(Replace this address with a mailbox you actually read before submitting, and keep it consistent with the support URL in App Store Connect.)
