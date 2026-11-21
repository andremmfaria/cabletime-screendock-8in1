script_args = getScriptArgs()
out_path = script_args[0] if script_args else "/tmp/ghidra_findings.txt"
f = open(out_path, "w")

fm = currentProgram.getFunctionManager()
sym_table = currentProgram.getSymbolTable()
targets = ["CreateProcessA", "ShellExecuteA", "ShellExecuteExA", "WinExec", "RegSetValueExA", "RegCreateKeyExA"]

from ghidra.app.decompiler import DecompInterface
decomp = DecompInterface()
decomp.openProgram(currentProgram)

refmgr = currentProgram.getReferenceManager()

for tname in targets:
    f.write("\n===== Callers of %s =====\n" % tname)
    syms = sym_table.getSymbols(tname)
    for sym in syms:
        addr = sym.getAddress()
        refs = refmgr.getReferencesTo(addr)
        caller_funcs = set()
        for ref in refs:
            fromAddr = ref.getFromAddress()
            func = fm.getFunctionContaining(fromAddr)
            if func is not None:
                caller_funcs.add(func)
        for func in caller_funcs:
            f.write("\n--- Caller function: %s @ %s ---\n" % (func.getName(), func.getEntryPoint()))
            res = decomp.decompileFunction(func, 30, monitor)
            if res.decompileCompleted():
                f.write(res.getDecompiledFunction().getC())
            else:
                f.write("(decompile failed: %s)\n" % res.getErrorMessage())

f.close()
print("WROTE: " + out_path)
