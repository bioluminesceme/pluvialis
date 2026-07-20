# Gemini PR protocol (Peregrine keyboard)

This is the first machine to implement (M4a), because the user's **Peregrine** speaks it and it gives us a real steno machine to test with on day one, long before the Luminex driver is installed.

Source: `F:\Steno\plover\plover\machine\gemini_pr.py` (56 lines, the whole thing). Serial plumbing is in `plover\machine\base.py::SerialStenotypeBase`.

---

## Transport

An emulated USB serial port (a COM port on Windows).

| Setting | Value |
|---|---|
| Baud | 9600 |
| Data bits | 8 |
| Parity | none |
| Stop bits | 1 |
| Flow control | none |

Rust crate: `serialport`. Enumerate ports with `serialport::available_ports()`.

**Port discovery for Auto mode:** there is no reliable way to know a COM port is a steno keyboard without opening it. Practical approach: on Windows, `serialport` reports USB VID/PID, so remember the VID/PID that worked and prefer it on later scans. Failing that, open candidate ports and watch for a valid packet (first byte with the high bit set, next five without) before declaring a match. Never hold a port open that is not producing steno, since that would block other software.

---

## Packet format

**Exactly 6 bytes per stroke.** The most significant bit is a framing marker, not data:

- byte 0 has MSB **set** (`0x80`)
- bytes 1 through 5 have MSB **clear**

So each byte carries 7 bits of key data, and 6 x 7 = 42 keys.

Validation, straight from the Python: discard the packet if `!(packet[0] & 0x80)` or if any of bytes 1..5 has its high bit set. A malformed packet means we are out of frame; resynchronise by scanning forward to the next byte with the high bit set.

## Key chart

42 entries, read as 6 rows of 7. For byte index `i` (0..6) and bit `j` (1..8, where bit `j` is tested as `b & (0x80 >> j)`), the key is `CHART[i * 7 + j - 1]`.

```
Fn    #1    #2    #3    #4    #5    #6
S1-   S2-   T-    K-    P-    W-    H-
R-    A-    O-    *1    *2    res1  res2
pwr   *3    *4    -E    -U    -F    -R
-P    -B    -L    -G    -T    -S    -D
#7    #8    #9    #A    #B    #C    -Z
```

```rust
for (i, b) in packet.iter().enumerate() {
    for j in 1..8 {
        if b & (0x80 >> j) != 0 {
            keys.push(CHART[i * 7 + j - 1]);
        }
    }
}
```

Note the duplicated physical keys: `S1-`/`S2-` are the two halves of the S key, `*1`..`*4` the four star keys, `#1`..`#C` the number bar segments. The **keymap layer** collapses these to system actions (`S-`, `*`, `#`), which is exactly why the keymap layer exists and must be built before this machine. Do not shortcut it by hardcoding the collapse here.

`Fn`, `pwr`, `res1`, `res2` are unmapped by default.

## Layout string

Plover expresses the machine's key set as a layout string, used to build the keymap:

```
#1 #2  #3 #4 #5 #6 #7 #8 #9 #A #B #C
Fn S1- T- P- H- *1 *3 -F -P -L -T -D
   S2- K- W- R- *2 *4 -R -B -G -S -Z
              A- O-       -E -U
pwr
res1
res2
```

---

## Why this one first

Beyond being testable hardware, Gemini PR is the simplest possible `Machine` implementation: open port, read 6 bytes, decode, emit. If the `Machine` trait cannot express this cleanly, the trait is wrong. It is a good shape test before the more demanding Stenograph implementation.
