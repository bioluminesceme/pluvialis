# Stenograph USB protocol (Luminex CSE)

Complete spec, transcribed from the working Python implementation. **You should not need to open the Python source to implement this.** If you do want it, it is at
`C:\Users\Corien\AppData\Local\plover\plover\plugins\win\Python313\site-packages\stenograph\`
(`transport_windows.py`, `packet.py`, `stroke.py`, `transport.py`, `exception.py`) and
`...\site-packages\plover_stenograph\base.py` for the read loop.

That implementation is `plover-stenograph` 2.1.1 by sammdot, and it is what currently drives the user's Luminex through the frozen Plover install. It works, so its protocol handling is trustworthy. Its *lifecycle* handling is not, see "Bugs we must not reproduce" below.

---

## 1. Finding and opening the device (Windows)

The writer is exposed by the Stenograph WDF driver as a device interface class.

```
Device interface class GUID: {202e68c5-5980-4a60-b761-77c4de9d5dbf}
```

**This is not the GUID written in the Python source, and that is not a typo.**
`transport_windows.py` says `{c5682e20-8059-604a-b761-77c4de9d5dbf}`, but it
builds the value with `GUID = wintypes.BYTE * 16` filled from
`uuid.UUID(...).bytes`, which is big endian. Win32's `GUID` is
`{DWORD, WORD, WORD, BYTE[8]}`, little endian in its first three fields, so the
first eight bytes arrive byte swapped. The swapped form above is what actually
reaches SetupAPI, and it is what works.

Verified on this machine rather than derived: the Luminex's `MI_00` interface is
registered under exactly this GUID in
`HKLM\SYSTEM\CurrentControlSet\Control\DeviceClasses`. To re-check on any
machine:

```powershell
Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Control\DeviceClasses' | ForEach-Object {
  $g = $_.PSChildName
  Get-ChildItem $_.PSPath -EA SilentlyContinue | Where-Object { $_.PSChildName -match 'VID_112B' } |
    ForEach-Object { "$g  $($_.PSChildName)" }
}
```

Do not confuse this with the driver INF's `ClassGuid`,
`{202e68c5-6980-4a60-b761-77c4de9d5dbf}` (note `6980`). That is the *setup
class*, a different thing from the *device interface* class, and one hex digit
apart. `wdfsgusb.inf` registers no interface GUID at all; the driver does it
internally.

Using the unswapped string enumerates nothing, and the failure looks exactly
like the writer being switched off.

Sequence:

1. `SetupDiGetClassDevsA(&guid, NULL, NULL, DIGCF_DEVICEINTERFACE | DIGCF_PRESENT)`
   - `DIGCF_DEVICEINTERFACE = 0x10`, `DIGCF_PRESENT = 0x02`
   - returns `INVALID_HANDLE_VALUE` on failure
2. `SetupDiEnumDeviceInterfaces(devinfo, NULL, &guid, 0, &iface_data)`
   - member index 0, we only ever use the first writer
   - failure with `GetLastError() == ERROR_NO_MORE_ITEMS (0x103)` means **no writer attached**. This is the normal "not plugged in" case and must be a clean, quiet, retryable outcome, not an error spam.
3. `SetupDiGetDeviceInterfaceDetailA(devinfo, &iface_data, NULL, 0, &required_size, NULL)`
   - expected to fail with `ERROR_INSUFFICIENT_BUFFER (0x7A)`; that is how you learn the buffer size
4. Allocate `required_size`, set `cbSize`, call again to get the device path
   - **`cbSize` gotcha:** it is the size of the *struct*, never the allocated buffer. Measured, not assumed: `ctypes.sizeof` of the Python's struct is **8** on x64 (a `DWORD` plus one `CHAR`, padded to 4 byte alignment), and 5 on x86 where the struct is packed. Use `size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_A>()`, which the `windows` crate gets right on both. Passing the buffer size yields `ERROR_INVALID_USER_BUFFER`, whose message hints at nothing.
   - **The path starts at offset 4**, not at the struct's padded size. Use `offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_A, DevicePath)`.
   - Allocate the buffer as `Vec<u32>`, not `Vec<u8>`: `cbSize` is written through that pointer and a misaligned `u32` write is undefined behaviour.
5. `SetupDiDestroyDeviceInfoList(devinfo)`
6. `CreateFileA(device_path, GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, 3, FILE_ATTRIBUTE_NORMAL, NULL)`
   - The Python passes `CREATE_ALWAYS | CREATE_NEW` which is `2 | 1 == 3`, and 3 is `OPEN_EXISTING`. It works because the numbers coincide. **Write `OPEN_EXISTING` in Rust and say so.**

Reads and writes are then plain synchronous `ReadFile` / `WriteFile` on that handle. No overlapped I/O, no ioctls.

---

## 2. Packet format

Header is 32 bytes, little-endian, no padding (Python `struct` format `<2sIH6I`):

| Offset | Size | Field |
|---|---|---|
| 0 | 2 | sync, always ASCII `"SG"` |
| 2 | 4 | sequence number, u32 |
| 6 | 2 | packet type, u16 |
| 8 | 4 | data length, u32 |
| 12 | 4 | p1 |
| 16 | 4 | p2 |
| 20 | 4 | p3 |
| 24 | 4 | p4 |
| 28 | 4 | p5 |

Followed by `data_length` bytes of payload.

- **Sequence number** increments per request, wrapping at `0xFFFFFFFF`. A response must echo the request's sequence number and packet type; if it does not, that is a protocol violation and the connection should be reset.
- **Payload is padded to a multiple of 8 bytes** when sending. The pad bytes are zero and are not counted in... actually in the Python, `data_length` is computed *after* padding, so the padding **is** included in `data_length`. Match that.

### Packet types

| Value | Name |
|---|---|
| `0x06` | ERROR |
| `0x11` | OPEN_FILE |
| `0x13` | READ_FILE |

### Error codes (in `p1` of an ERROR packet)

| Value | Meaning | How to react |
|---|---|---|
| 3 | UNABLE_TO_PERFORM | reset read state, keep going |
| 7 | FILE_NOT_AVAILABLE | reset read state, keep going |
| 8 | NO_REALTIME_FILE | **normal.** User has not started writing yet. Reset state and keep polling. Not an error to show the user. |
| 9 | FINISHED_READING_CLOSED_FILE | **normal.** The file was closed. Reset state, reopen the realtime file. |

Only codes 8 and 9 happen routinely. Treating 8 as a failure is a common way to build something that looks broken when it is merely idle.

### Requests we send

**Open the realtime file:**
```
packet_type = 0x11 (OPEN_FILE)
p1          = 0x41  (ASCII 'A', the disk id)
data        = b"REALTIME.000"   (12 bytes, zero-padded to 16)
```

**Read from the file:**
```
packet_type = 0x13 (READ_FILE)
p1          = file offset (starts at 0, advances by response.data_length)
p2          = byte count = 0x200 (MAX_READ)
```

---

## 3. Decoding strokes

A READ_FILE response payload is a sequence of **8-byte chords**: 4 bytes of steno, then 4 bytes of timestamp (the timestamp is unused, discard it). `data_length` is always a multiple of 8.

Each of the 4 steno bytes carries **6 key bits in its low 6 bits**, and the top two bits are always set (the Python asserts `byte >= 0b11000000`). Bit 5 (value 32) is the first key in the row, bit 0 (value 1) is the last:

```
row 0 (byte 0):  ^    #    S-   T-   K-   P-
row 1 (byte 1):  W-   H-   R-   A-   O-   *
row 2 (byte 2):  -E   -U   -F   -R   -P   -B
row 3 (byte 3):  -L   -G   -T   -S   -D   -Z
```

```rust
// for each row index i in 0..4, byte b:
for j in 0..6 {
    if b & (1 << (5 - j)) != 0 {
        keys.push(CHART[i][j]);
    }
}
```

Note `^` and `#` are machine keys that the keymap layer maps to system actions; they are not steno letters. The keymap this machine uses is Plover's **Stentura** keymap.

---

## 4. The read loop

This is the logic to reproduce, from `plover_stenograph/base.py::run()`. State is three fields:

```
realtime:           false until we get a zero-length response (means "caught up to live")
realtime_file_open: false until an OPEN_FILE succeeds
offset:             file read offset
```

Loop, until told to stop:

1. If `!realtime_file_open`: send OPEN_FILE, set `realtime_file_open = true`.
2. Send READ_FILE at `offset`.
3. On `NoRealtimeFile` or `FinishedReadingClosedFile`: **reset all state**, continue. (Do not treat as failure.)
4. On any I/O or connection error: reset all state, drop to the reconnect loop, log the warning **once** rather than every attempt.
5. On success:
   - if `data_length > 0`: `offset += data_length`
   - else if `!realtime`: we have caught up to live, set `realtime = true` and report Ready
   - if `data_length > 0 && realtime`: decode and emit the strokes
   - if `realtime`: sleep 100ms before the next poll

Strokes read while `!realtime` are deliberately discarded: they are the backlog already in the file, and emitting them would dump old text into the document on connect.

---

## 5. Bugs we must not reproduce

These are the reason this project exists. All three are in the Python and all three are avoidable.

1. **`start_capture()` gives up permanently.** If the writer is absent at the moment capture starts, it calls `_error()` and never starts the reader thread. Nothing ever retries. This is the root cause of the user's ritual: restart the writer, press a key, reopen settings, re-select the machine. **Our design: the connect loop runs unconditionally in the background and retries forever, once per second. Absent hardware is a state, not a failure.**

2. **`disconnect()` leaks the handle.** It sets `self._usb_device = INVALID_HANDLE_VALUE` and *then* calls `CloseHandle` on that now-invalid value. The real handle is never closed, and `CloseHandle(-1)` fails, so it also raises. Over a long retry loop this exhausts handles.
   ```rust
   // correct: take the handle, then close it
   let h = std::mem::replace(&mut self.handle, INVALID_HANDLE_VALUE);
   if h != INVALID_HANDLE_VALUE { unsafe { CloseHandle(h); } }
   ```
   The M4b soak test (ten minutes of failed connects, handle count flat) exists specifically to prove we did not repeat this.

3. **`connect()` returns `false` instead of erroring** when no device is found, so callers that check for an exception see success. Our `connect()` returns `Result` and "no device present" is a distinct, quiet variant that the retry loop expects.

4. **An error response is misread as a protocol violation, which kills the reader thread.** Found on 2026-07-20 while implementing this. `send_receive` only calls `handle_response` when the response echoes the request's sequence number *and* packet type:
   ```python
   if (writer_packet.sequence_number == request.sequence_number and
       writer_packet.packet_type == request.packet_type):
       return self.handle_response(writer_packet)
   raise ProtocolViolationException()
   ```
   But an ERROR packet's type is `0x06`, which never equals `READ_FILE` (`0x13`). So `handle_response` is unreachable for errors, `NoRealtimeFileException` can never be raised, and the `except NoRealtimeFileException` in `base.py::run()` is dead code. What is raised instead is `ProtocolViolationException`, which `run()` does not catch at all, so it propagates out and ends the thread permanently.

   Error code 8 means "the user has not started writing yet", which is the writer's state most of the time and certainly at startup. That makes this trivially easy to hit and is a strong candidate for the user's actual symptom.

   **Our handling:** check `is_error` *first* and dispatch on the code. The packet is self-describing, so the type check cannot and should not apply to it, and a mismatched sequence on an error is logged rather than fatal. Sequence and type are still enforced strictly for non-error responses. And because every protocol violation resets the connection rather than ending anything, being wrong about this costs a reconnect, not a dead session.

---

## 6. Driver

**On this machine the driver is already installed and nothing needs running.** Verified 2026-07-20: `Steno Machine` at `USB\VID_112B&PID_000D&MI_00` is bound to service `wdfsgusbV3`, status OK, no error code. This section previously said installing it was a milestone M8 step; it was not needed then and is not needed now.

The writer still needs that driver to enumerate under the interface GUID, so for a fresh machine the installer is here:

```
F:\Steno\StenoMachines\USB_Writer_Drivers\USB Writer Drivers\Drivers\
  StenographDriverInstall.exe
  RemoveDrivers.exe
  SGSerial.inf, wdfsgusb.inf, wdfsgusb.sys, .cat catalogs
```

`wdfsgusb.inf` binds `USB\VID_112B&PID_000D&MI_00` plus a dozen older Stenograph models. Note it registers **no device interface GUID**; the driver does that internally, which is why the INF cannot be used to look the GUID up. Its `ClassGuid` is the *setup* class and is one hex digit away from the interface GUID, so it is an easy and costly thing to grab by mistake.

**The writer must be powered on to enumerate.** Switched off, no device nodes appear at all, so absence proves nothing about the driver.

**Close official Plover before testing.** Two programs cannot hold the writer handle at once.
