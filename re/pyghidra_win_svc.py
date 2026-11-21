import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USBDisplayService.exe"
out_path = "/tmp/svc_findings.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    refmgr = program.getReferenceManager()
    st = program.getSymbolTable()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        # callers of hid_open / hid_write / hid_read / hid_send_feature_report / hid_get_input_report
        for name in ["hid_open", "hid_open_path", "hid_write", "hid_read", "hid_read_timeout",
                     "hid_send_feature_report", "hid_get_input_report", "hid_get_feature_report",
                     "hid_enumerate", "hid_init"]:
            syms = st.getSymbols(name)
            f.write("\n\n===== Callers of %s (nsyms=%d) =====\n" % (name, len(list(syms))))
            syms = st.getSymbols(name)
            for sym in syms:
                fn_target = fm.getFunctionAt(sym.getAddress())
                f.write("symbol %s @ %s is_function=%s\n" % (sym.getName(), sym.getAddress(), fn_target is not None))
                refs = refmgr.getReferencesTo(sym.getAddress())
                callers = set()
                for r in refs:
                    cfn = fm.getFunctionContaining(r.getFromAddress())
                    if cfn:
                        callers.add(cfn)
                for cfn in callers:
                    f.write("\n--- caller %s @ %s ---\n" % (cfn.getName(), cfn.getEntryPoint()))
                    res = decomp.decompileFunction(cfn, 30, flat_api.getMonitor())
                    if res.decompileCompleted():
                        f.write(res.getDecompiledFunction().getC()[:6000])
                    else:
                        f.write("(failed: %s)\n" % res.getErrorMessage())

print("DONE", out_path)
