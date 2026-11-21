import pyghidra
pyghidra.start()
from ghidra.app.decompiler import DecompInterface

binpath = "/home/andremmfaria/projects/cabletime-screendock-8in1/re/win_real/USB Display.exe"
out_path = "/tmp/gui_reset_deep.txt"

# string addrs found by gui7_json.py scan
TARGETS = {
    "heartbeat": 0x00908994,
    "HEARTBEAT_TIMEOUT": 0x00908fbc,
    "heartbeat_fmt6": 0x009090a4,
    "heartbeat_fmt_full": 0x009090d0,
    "USBHeartbeatThread_name": 0x00909118,
    "reset_json": 0x00909150,
    "HID_USB_heartbeat_end": 0x009091a0,
    "USBHeartbeatThread_exception": 0x009091b8,
    "Do_Heartbeat_name": 0x009091e8,
    "jsonrpc": 0x0090a0e0,
    "Heartbeat_bracket": 0x0090a860,
    "NetworkSocket_Do_Heartbeat": 0x0090a8dc,
}

with pyghidra.open_program(binpath, analyze=True) as flat_api:
    program = flat_api.getCurrentProgram()
    fm = program.getFunctionManager()
    refmgr = program.getReferenceManager()
    af = program.getAddressFactory()
    decomp = DecompInterface()
    decomp.openProgram(program)

    with open(out_path, "w") as f:
        seen_funcs = set()
        for label, addr_int in TARGETS.items():
            addr = af.getAddress(hex(addr_int))
            refs = refmgr.getReferencesTo(addr)
            f.write("\n=== refs to %s @ %s ===\n" % (label, addr))
            count = 0
            for r in refs:
                count += 1
                cfn = fm.getFunctionContaining(r.getFromAddress())
                if cfn:
                    f.write("  from %s in function %s @ %s\n" % (r.getFromAddress(), cfn.getName(), cfn.getEntryPoint()))
                    seen_funcs.add(cfn)
                else:
                    f.write("  from %s (no containing function)\n" % r.getFromAddress())
            f.write("  (total refs: %d)\n" % count)

        f.write("\n\n===== FULL DECOMPILES OF ALL REFERENCING FUNCTIONS =====\n")
        for cfn in seen_funcs:
            f.write("\n\n--- %s @ %s ---\n" % (cfn.getName(), cfn.getEntryPoint()))
            res = decomp.decompileFunction(cfn, 60, flat_api.getMonitor())
            if res.decompileCompleted():
                f.write(res.getDecompiledFunction().getC()[:15000])
            else:
                f.write("(decompile failed: %s)\n" % res.getErrorMessage())

print("DONE", out_path)
