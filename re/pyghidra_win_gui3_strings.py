import pyghidra
pyghidra.start()

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_strings_scan.txt"

KEYWORDS = [
    "MI_02", "MI_03", "MI_04", "COL02", "COL03", "COL04",
    "audio", "Audio", "AUDIO",
    "storage", "Storage", "STORAGE", "mass storage", "MassStorage",
    "cdrom", "CDROM", "CdRom", "cd-rom",
    "brightness", "Brightness",
    "power", "Power", "POWER",
    "EDID", "edid",
    "resolution", "Resolution",
    "hub", "Hub", "HUB",
    "port", "Port",
    "button", "Button",
    "USBSTOR", "usbstor",
    "wdma_usb",
    "SD card", "TF card", "card reader", "CardReader",
    "Ethernet", "RJ45", "LAN",
    "PD", "charging", "Charging",
]

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    listing = program.getListing()

    with open(out_path, "w") as f:
        data_iter = listing.getDefinedData(True)
        count = 0
        while data_iter.hasNext():
            data = data_iter.next()
            dt_name = data.getDataType().getName().lower()
            if "string" not in dt_name and "unicode" not in dt_name:
                continue
            try:
                val = data.getValue()
            except Exception:
                continue
            if not isinstance(val, str) or len(val) < 2:
                continue
            for kw in KEYWORDS:
                if kw in val:
                    addr = data.getAddress()
                    f.write("%s | %r\n" % (addr, val[:200]))
                    count += 1
                    break
        f.write("\nTOTAL MATCHES: %d\n" % count)

print("DONE", out_path)
