import os, fcntl, struct, time, sys

def HIDIOCSFEATURE(length):
    # _IOC(_IOC_WRITE|_IOC_READ, 'H', 0x06, length)
    IOC_WRITE, IOC_READ = 1, 2
    dirn = IOC_WRITE | IOC_READ
    return (dirn << 30) | (length << 16) | (ord('H') << 8) | 0x06

def send_feature(node, report_id, data_bytes, label):
    buf = bytes([report_id]) + data_bytes
    fd = os.open(node, os.O_RDWR)
    try:
        arr = bytearray(buf)
        try:
            fcntl.ioctl(fd, HIDIOCSFEATURE(len(arr)), arr)
            print(label, "SFEATURE report", report_id, data_bytes.hex(), "-> OK")
        except OSError as e:
            print(label, "SFEATURE report", report_id, data_bytes.hex(), "-> ERROR", e)
    finally:
        os.close(fd)

print("=== Trying Feature report 0x20 handshake candidates ===")
for node in ["/dev/hidraw0", "/dev/hidraw1"]:
    for candidate in [bytes([0x01, 0x00]), bytes([0x00, 0x01]), bytes([0x01, 0x01])]:
        send_feature(node, 0x20, candidate, node)
        time.sleep(0.3)

print("\n=== Resending H264 test frame with endpoint-correct chunk sizes ===")
DATA = open("/home/andremmfaria/projects/cabletime-screendock-8in1/re/test_frame.h264", "rb").read()

def write_chunks(node, report_id, max_payload, label):
    fd = os.open(node, os.O_RDWR)
    try:
        n = 0
        for off in range(0, len(DATA), max_payload):
            chunk = DATA[off:off+max_payload]
            pkt = bytes([report_id]) + chunk
            try:
                os.write(fd, pkt)
            except OSError as e:
                print(label, "write failed at offset", off, "len", len(chunk), ":", e)
                return
            n += 1
        print(label, "-> sent", n, "chunks of <=", max_payload, "bytes,", len(DATA), "total")
    finally:
        os.close(fd)

print("\n--- report 1 via hidraw0, 63-byte chunks (matches its 64B max packet) ---")
write_chunks("/dev/hidraw0", 1, 63, "report1/63B")
time.sleep(2)

print("\n--- report 2 via hidraw1, 760-byte chunks (matches its 768B max packet) ---")
write_chunks("/dev/hidraw1", 2, 760, "report2/760B")
time.sleep(2)

print("\nDone. Please check the dock's physical screen now.")
