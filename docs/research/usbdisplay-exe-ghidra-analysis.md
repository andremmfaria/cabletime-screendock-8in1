# USBDisplay.exe static analysis (Ghidra + supporting tools)

Raw research pass. Binary pulled from the dock's own virtual CD gadget (sr0), analyzed on this host. Workdir: `re/` at the project root (original exe, UPX-unpacked exe, Ghidra project, dump script). Binary itself is not committed anywhere beyond this local project dir; treat as a vendor artifact, not something to redistribute.

## 1. Unpacking

Original `USBDisplay.exe` is UPX-packed (confirmed earlier pass). Unpacked clean with `upx -d` (97.24% ratio reported, standard for UPX). Unpacked copy: `USBDisplay_unpacked.exe`, PE32/i386, 8 sections.

Unpacking the binary strips/invalidates its Authenticode signature (expected, this is destructive to signature validation, not a finding about the vendor). Signature was separately verified against the untouched original (see section 3).

## 2. What the binary actually is

`strings` on the unpacked binary surfaces this is built with **Ghost Installer Studio** (Ethalone), a generic commercial Windows installer-authoring toolkit from the early 2000s, still used by small hardware vendors for driver/app packages. Evidence: literal manifest description `"Ghost Installer"`, registry key `HKLM\SOFTWARE\Ethalone\Ghost Installer\2.0`, and a full set of `TInstallAction`/`CM_*`/`EV_*` class and event names matching that toolkit's scripting engine (install actions, suspend/resume/abort install events, install-type selection).

Practical meaning: the installer's `CreateProcessA`/`ShellExecuteA`/`RegCreateKeyExA`/`RegSetValueExA` imports are generic Ghost Installer runtime machinery for running configurable "install actions" (launching bundled files, writing registry keys for the real app's autostart entry), not bespoke vendor code. Two bundled payload files are referenced by name in error strings: `comregc.exe` and `unpack.dll`, both failure messages ("Can't extract ...") from the installer's own self-extraction step. These are packed inside the installer's data/resources and were not extracted for this pass (would require either running the installer on a real or virtualized Windows host, or reverse-engineering Ghost Installer's package format, neither attempted here).

## 3. Code-signing (correction to the earlier security-assessment.md draft)

Earlier pass wrongly stated "no Authenticode signature found" — that check only ran `strings` against the binary and never looked for a binary PKCS#7 blob. Re-checked properly with `osslsigncode verify` against the **original** (not unpacked) exe:

```
Number of verified signatures: 2
Signature verification: ok (both SHA1 and SHA256 signatures)
Signer: CN=Actions Microelectronics Co., Ltd., O=Actions Microelectronics Co., Ltd., L=Shenzhen, ST=Guangdong Province, C=CN
Issuer chain: DigiCert Trusted G4 Code Signing RSA4096 SHA384 2021 CA1 -> DigiCert Trusted Root G4
Signing/timestamp time: May 13 2025 06:59:53/57 GMT (RFC3161 countersigned, timestamp verification ok)
Leaf cert validity window: 2023-09-15 to 2026-09-25 (expired now relative to today, but the RFC3161 timestamp proves the signature was made while the cert was valid — this is the normal/correct way Authenticode handles cert expiry, not a red flag)
```

This is a legitimately signed vendor binary, dual-signed for older and newer Windows signature-verification compatibility. Revises the earlier "unsigned, treat with suspicion" framing downward substantially — this is routine vendor driver-package signing, not an anonymous dropper.

`binwalk` on the unpacked (unsigned, because unpacking stripped it) copy flagged two "JBOOT STAG header" hits with absurd claimed image sizes (469MB and 1.2GB inside a 7.4MB file) — these are false-positive byte-pattern matches on unrelated data, not real embedded firmware images. Noted so a future pass doesn't chase them.

## 4. Ghidra: can this binary make the host run arbitrary things?

Ran `analyzeHeadless` (full auto-analysis, ~49s) then a custom post-script (`re/ghidra_dump.py`) to decompile every function that calls `CreateProcessA`, `ShellExecuteA`, `ShellExecuteExA`, `WinExec`, `RegSetValueExA`, `RegCreateKeyExA` and dump their callers.

Result: Ghidra could only resolve the import thunks themselves as "callers" — the actual call sites inside the installer's logic use indirect/virtual dispatch Ghidra couldn't statically trace (`WARNING: Could not recover jumptable... Too many branches`). This is consistent with the binary being Borland/Delphi-style Pascal code (confirms the `"Copyright (c) 1983,97 Borland"` string found earlier) where the Ghost Installer engine dispatches "install actions" through a virtual method table rather than direct calls — a known Ghidra limitation on this class of binary, not evidence of deliberate obfuscation or a hidden capability.

**Direct answer to "can we run things from the device itself":** no autonomous/remote execution path was found. What exists is the standard chain already flagged in the first security pass: dock enumerates a virtual CD -> Windows AutoPlay offers to run the signed installer -> installer's generic action engine extracts and launches bundled files (the real display driver/app) and writes a `Run`/`RunOnce` autostart key for the resulting tray app. Every step of that chain requires either user consent (AutoPlay click-through) or the installer completing normally; nothing here lets the dock's firmware trigger code execution on the host by itself, and the HID vendor channel (checked in the first pass) is a one-way opaque data pipe with no keyboard/mouse usage pages, so it cannot inject input either.

## Open items if this gets revisited

- Extract and inspect the two embedded payload files (`comregc.exe`, `unpack.dll`) from inside the Ghost Installer package — would need either a Windows/Wine sandbox to let the installer self-extract, or a Ghost Installer package-format parser.
- Confirm whether the resulting installed tray app (not the installer itself) has any additional capability — this pass only covered the installer wrapper, not what it actually deploys.
- SHA-256 of the original signed exe was computed in the first pass but not submitted to VirusTotal (ask before doing this, it discloses the hash externally).
