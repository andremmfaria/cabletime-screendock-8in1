Session log: setting up Windows-side access for the dock research, 2026-11-18.

Context: the user accesses this Linux research host over SSH from a Windows PC
(hostname azul, LAN IP 192.168.10.166, UniFi VLAN 10 "user-devices"). The dock
research had stalled on the point recorded in
docs/research/windows-mac-driver-correlation.md section 4: the real Windows
USBDisplay driver was never captured because only the installer wrapper was
examined, and resolving the HID packet-framing question needs either a live
USB capture against the real hardware or an interactive Ghidra trace. This
session set up remote access to the Windows box so that capture can happen.

What was done, in order:

1. Enabled OpenSSH Server on Windows via the OpenSSH.Server Windows capability
   (Add-WindowsCapability), started and enabled the sshd service, and
   confirmed the OpenSSH-Server-In-TCP firewall rule was present and enabled.
   One early hiccup: the capability's version suffix in this chat kept
   arriving stripped of its tilde characters, because a chat markdown
   renderer was treating the double-tilde sequence as strikethrough syntax
   and deleting it. Worked around by having the user pull the exact capability
   name object from Get-WindowsCapability output directly in PowerShell rather
   than retyping or pasting it from chat.

2. Created a dedicated local admin account, dockresearch, for this work,
   added to the Administrators group, rather than using the user's own daily
   account. The user chose to set a known password directly ("dockresearch001")
   and have it used as-is for scripted SSH commands for the duration of this
   session, with the account to be disabled/deleted afterward. That password
   is therefore already a disclosed/disposable secret from this point on, not
   a real credential, and should not be reused for anything else.

3. Hit a network-reachability problem: this Linux host sits on UniFi VLAN 50
   (server, 192.168.50.0/24) and the Windows box sits on VLAN 10
   (user-devices, 192.168.10.0/24). Initial SSH and ICMP both timed out with
   100% packet loss. Checked the UniFi gateway (kantharos-udm-pro) directly
   over its API: neither VLAN had network_isolation_enabled set, and the only
   two custom firewall policies on the site (a DNS-allow for isolated
   networks and a DoT-853 block) do not touch either VLAN or port 22 -
   confirmed the gateway was not the cause. A subsequent retry succeeded
   cleanly and five back-to-back connection attempts all worked, so the
   original timeout is judged to have been a one-off transient blip (e.g. ARP
   warm-up or a momentary link flap), not a real block. Should this
   intermittency resurface, re-check the UniFi gateway state again rather
   than assuming the first clean diagnosis still holds.

4. Installed Wireshark and USBPcap on the Windows box over the new SSH
   session, run non-interactively as the dockresearch user:
   - winget was present (Microsoft.DesktopAppInstaller 1.30.310.0, on PATH)
     but its command alias did not resolve in this non-interactive SSH
     context, and the expected per-user WindowsApps\winget.exe alias stub did
     not exist for this account - consistent with that alias normally being
     created on a user's first interactive logon, which this freshly created
     service account had never done. Rather than force an interactive logon
     just to unblock winget, downloaded and ran the official installers
     directly instead.
   - Wireshark: downloaded the official 64-bit NSIS installer from
     2.na.dl.wireshark.org and ran it silently (/S /desktopicon=no
     /quicklaunchicon=no). Confirmed Wireshark.exe present under
     C:\Program Files\Wireshark afterward.
   - USBPcap: Wireshark's own silent install does not cascade into USBPcap's
     separate sub-installer, so USBPcap was fetched and installed separately,
     direct from its GitHub release (desowin/usbpcap, latest tagged 1.5.4.0),
     run silently (/S). Confirmed USBPcapCMD.exe present under
     C:\Program Files\USBPcap, but the USBPcap kernel driver did not yet show
     up as a running service - per the project's own documentation, a reboot
     is required after install for the driver to bind. A reboot was pending
     the user's own timing choice as of the end of this session.

Open next steps (for whoever picks this up): reboot the Windows box, confirm
the USBPcap service comes up, physically connect the CABLETIME dock to that
machine, run the real USBDisplay.exe installer (not just inspect it, this
time let it actually drop its driver binary to disk), then capture USB
traffic with USBPcap/Wireshark while that driver talks to the dock. That
capture is what resolves the long-open HID packet-framing question described
in docs/research/display-protocol-reverse-engineering.md and
docs/research/windows-mac-driver-correlation.md. Once the real Windows driver
binary exists on disk, it is also worth the same pyghidra-based decompile
treatment already given to the Mac app.

Housekeeping: the dockresearch account is meant to be temporary. Disable it
when not actively in use (Disable-LocalUser -Name 'dockresearch') and remove
it entirely once this phase of the research concludes (Remove-LocalUser -Name
'dockresearch'), since its password was deliberately handled as a disposable,
already-disclosed secret rather than a real credential.
