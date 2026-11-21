import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_requireencoder.txt"

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    st = program.getSymbolTable()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        # 1. Find RequireEncoder function(s) by name match
        f.write("=== functions matching 'RequireEncoder' ===\n")
        for fn in fm.getFunctions(True):
            if "RequireEncoder" in fn.getName():
                f.write("FUNC %s @ %s\n" % (fn.getName(), fn.getEntryPoint()))
                res = decomp.decompileFunction(fn, 60, flat_api.getMonitor())
                if res.decompileCompleted():
                    f.write(res.getDecompiledFunction().getC()[:12000])
                else:
                    f.write("(decompile failed: %s)\n" % res.getErrorMessage())

        # 2. Search for NVENC/NVIDIA/CUDA encoder class names anywhere in symbol table
        f.write("\n\n=== symbols containing NV/NVENC/CUDA/NVIDIA ===\n")
        for kw in ["NVENC", "NvEnc", "nvEnc", "NVIDIA", "CUDA", "Cuda"]:
            for s in st.getAllSymbols(True):
                if kw in s.getName():
                    f.write("SYM %s @ %s\n" % (s.getName(), s.getAddress()))

        # 3. All functions with class-qualified names containing Encoder
        f.write("\n\n=== all functions with 'Encoder' in name ===\n")
        for fn in fm.getFunctions(True):
            if "Encoder" in fn.getName():
                f.write("FUNC %s @ %s\n" % (fn.getName(), fn.getEntryPoint()))

print("DONE", out_path)
