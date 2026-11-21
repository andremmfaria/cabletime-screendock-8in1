import pyghidra
pyghidra.start()

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USBDisplayService.exe"
out_path = "/tmp/svc_strings_scan.txt"

KEYWORDS = [
    "MI_02", "MI_03", "MI_04", "COL02", "COL03", "COL04",
    "audio", "Audio", "AUDIO",
    "storage", "Storage", "STORAGE",
    "cdrom", "CDROM", "CdRom",
    "brightness", "Brightness",
    "power", "Power", "POWER",
    "EDID", "edid",
    "resolution", "Resolution",
    "hub", "Hub", "HUB",
    "button", "Button",
    "USBSTOR", "usbstor",
    "heartbeat", "jsonrpc", "json",
    "method",
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
                    f.write("%s | %r\n" % (addr, val[:300]))
                    count += 1
                    break
        f.write("\nTOTAL MATCHES: %d\n" % count)

print("DONE", out_path)
