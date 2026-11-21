Session log: USB enumeration check on azul, 2026-11-21.

Context: following up on docs/research/windows-host-access-session.md. The user enabled sshd and the dockresearch service user on azul (192.168.10.166, VLAN 10) again; this session connected from the Linux research host (vermelho, VLAN 50) and checked how the dock enumerates.

Connectivity note: ICMP to 192.168.10.166 still shows 100% loss (as before), but TCP/22 and SSH auth work fine. ICMP appears blocked while SSH is not; do not use ping alone to judge reachability on this VLAN pairing.

Dock USB enumeration (Get-PnpDevice):
The dock shows up as a single USB composite device, VID_1DE1&PID_F201 (VIA Labs vendor ID), parent instance USB\VID_1DE1&PID_F201\84832A9F, with four child interfaces, all Status OK:
- MI_00: HID vendor-defined device + a Microsoft Input Configuration Device collection
- MI_01: HID vendor-defined device + a Microsoft Input Configuration Device collection
- MI_02: MEDIA class, "USB Display Audio (HDMI 1)" (Microsoft driver) - this carries audio for the dock's screen/HDMI output
- MI_04: USB Mass Storage Device ("Compatible USB storage device") - almost certainly the autorun/driver-CD partition typical of these docks

No USB Video Class or generic "USB Display" device class appears under this VID/PID. The actual screen surfaces separately as a Display-class device:
{0511CEC1-0EB4-4ADE-9284-CD36606C3906}\VID_ACTIONSMICRO_PID_EXTENSION_VIRTUAL_DISPLAY_0001\18193
FriendlyName "extension Graphics Adapter", Manufacturer "Actions Microelectronics Co., Ltd.", DriverVersion 1.1.303.136, Status OK. Its bus parent is ROOT\EXTENSION_VIRTUAL_BUS\0000, class System, FriendlyName "extension virtual Bus".

This is a meaningful correction to docs/overview.md's assumption. Overview.md states the screen "is recognized by the host computer as a real secondary monitor (via DisplayPort Alt Mode over the USB-C connection)". What's actually on this box is a Windows Indirect Display Driver (IddCx) virtual bus/adapter from Actions Microelectronics, not a plain DP Alt Mode GPU-native monitor. That means the picture path is software-composited by the Actions Micro driver and shipped to the dock over the vendor HID channel (or possibly a separate bulk/isoc path not surfaced as its own PnP node), rather than the GPU driving a real external DP sink directly. This matches the "no OS, driver required" framing in overview.md but is a more specific mechanism worth folding into docs/research/display-protocol-reverse-engineering.md and docs/research/mac-driver-analysis.md for cross-checking against the Mac side.

No errored/failed devices (Status NOT OK) were found anywhere in the PnP device list on this machine - the dock and all its interfaces are enumerating and binding drivers cleanly, so there is no basic enumeration problem blocking the next research step.

USBPcap status: the box has rebooted since the last session (LastBootUpTime 2026-11-21 11:23:08) and C:\Program Files\USBPcap contains USBPcap.sys, USBPcap.inf, USBPcapCMD.exe, usbpcapamd64.cat, Uninstall.exe - but no USBPcap kernel-mode service/driver shows up running (checked both Get-Service and Win32_SystemDriver, no match). So the reboot alone did not bring the capture driver up; it likely needs an explicit driver install/registration step (e.g. re-running the installer, or pnputil install of USBPcap.inf, or Windows asking for a driver-signing/consent step in an interactive session) that a headless SSH session cannot satisfy on its own.

Next steps (for whoever picks this up):
1. Get the USBPcap kernel driver actually registered and running - this likely needs an interactive logon (RDP/console), not SSH, since driver installation sometimes requires UI consent and this is unusually not auto-started after reboot despite the installer reporting success previously.
2. Once USBPcapCMD lists an interface, capture while physically plugging/unplugging the dock and while running the real USBDisplay.exe installer end to end (per the open item already logged in windows-host-access-session.md).
3. Target capture at the two vendor HID interfaces (MI_00, MI_01 on VID_1DE1&PID_F201) and at whatever transport feeds the Actions Micro IddCx adapter - that pairing is what should resolve the HID packet-framing question.
4. Fold the IddCx/Actions Micro correction into overview.md and the two driver-analysis docs so they stop describing this as plain DP Alt Mode.

Housekeeping: dockresearch account and its disposable password remain as documented in windows-host-access-session.md; no account changes were made this session.
