import pyghidra
pyghidra.start()

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_symbols_scan.txt"

PREFIXES = ["EZDisplayTransmitter", "USBUtil", "DisplayUtil", "EZDisplayReceiver",
            "USBDisplay", "CEZMiraDlg::OnUSB", "HidD_", "hid_"]

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()

    with open(out_path, "w") as f:
        funcs = fm.getFunctions(True)
        matched = []
        for fn in funcs:
            name = fn.getName()
            for p in PREFIXES:
                if name.startswith(p):
                    matched.append((name, fn.getEntryPoint()))
                    break
        matched.sort()
        for name, addr in matched:
            f.write("%s @ %s\n" % (name, addr))
        f.write("\nTOTAL: %d\n" % len(matched))

print("DONE", out_path)
