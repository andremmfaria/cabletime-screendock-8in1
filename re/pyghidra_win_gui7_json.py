import pyghidra
pyghidra.start()

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_json_scan.txt"

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
            if "jsonrpc" in val or '"method"' in val or 'heartbeat' in val.lower() or '"params"' in val:
                addr = data.getAddress()
                f.write("%s | %r\n" % (addr, val[:300]))
                count += 1
        f.write("\nTOTAL MATCHES: %d\n" % count)

print("DONE", out_path)
