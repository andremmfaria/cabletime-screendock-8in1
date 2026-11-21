import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_findings.txt"

TARGET_STRINGS = [
    "EZDisplayTransmitter::SendHID",
    "EZDisplayTransmitter::SendTCP",
    "EZDisplayTransmitter::PrepareH264Encoder",
    "EZDisplayTransmitter::SetVideoBuffer",
    "EZDisplayTransmitter::send_thread",
    "EZDisplayTransmitter::LogFrameRate",
    "VID_1DE1&PID_F201",
    "MI_01",
    "MI_00",
]

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    refmgr = program.getReferenceManager()
    listing = program.getListing()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        data_iter = listing.getDefinedData(True)
        while data_iter.hasNext():
            data = data_iter.next()
            dt_name = data.getDataType().getName().lower()
            if "string" not in dt_name and "unicode" not in dt_name:
                continue
            try:
                val = data.getValue()
            except Exception:
                continue
            if not isinstance(val, str):
                continue
            for target in TARGET_STRINGS:
                if target in val:
                    addr = data.getAddress()
                    f.write("\n\n===== STRING %r @ %s =====\n" % (val[:200], addr))
                    refs = refmgr.getReferencesTo(addr)
                    callers = set()
                    for r in refs:
                        cfn = fm.getFunctionContaining(r.getFromAddress())
                        if cfn:
                            callers.add(cfn)
                    for cfn in callers:
                        f.write("\n--- referencing function %s @ %s ---\n" % (cfn.getName(), cfn.getEntryPoint()))
                        res = decomp.decompileFunction(cfn, 40, flat_api.getMonitor())
                        if res.decompileCompleted():
                            f.write(res.getDecompiledFunction().getC()[:8000])
                        else:
                            f.write("(decompile failed: %s)\n" % res.getErrorMessage())
                    break

print("DONE", out_path)
