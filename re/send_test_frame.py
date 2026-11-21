import os, time, sys

DATA = open("/home/andremmfaria/projects/cabletime-screendock-8in1/re/test_frame.h264", "rb").read()
print("H264 test frame size:", len(DATA), "bytes")

def write_chunks(node, report_id, max_payload, label, header_fn=None):
    fd = os.open(node, os.O_RDWR)
    try:
        n_chunks = 0
        for off in range(0, len(DATA), max_payload):
            chunk = DATA[off:off+max_payload]
            if header_fn:
                pkt = bytes([report_id]) + header_fn(off, chunk) + chunk
            else:
                pkt = bytes([report_id]) + chunk
            try:
                os.write(fd, pkt)
            except OSError as e:
                print(label, "write failed at offset", off, ":", e)
                return
            n_chunks += 1
        print(label, "-> sent", n_chunks, "chunks,", len(DATA), "bytes total via", node, "report", report_id)
    finally:
        os.close(fd)

print("\n=== ATTEMPT A: hidraw0, report 1, raw chunks (<=4094B), no extra header ===")
write_chunks("/dev/hidraw0", 1, 4094, "A")
time.sleep(3)

print("\n=== ATTEMPT B: hidraw1, report 2, raw chunks (<=12286B), no extra header ===")
write_chunks("/dev/hidraw1", 2, 12286, "B")
time.sleep(3)

print("\n=== ATTEMPT C: hidraw1, report 2, chunks (<=760B) prefixed with 4-byte LE total length ===")
def len_header(off, chunk):
    return len(DATA).to_bytes(4, "little")
write_chunks("/dev/hidraw1", 2, 760, "C", header_fn=len_header)
time.sleep(3)

print("\nAll attempts sent. Please check the dock's physical screen now.")
