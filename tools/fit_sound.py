"""Fit a synth recipe to a reference recording, by numbers.

Workflow (used for the player gun, 2026-07-10):
  1. ffmpeg -i ref.mp3 -ac 1 -ar 22050 ref.wav
  2. Adjust the slice below to isolate one clean hit in the reference.
  3. Edit candidate() — same primitives as vex_audio::synth, mirrored
     exactly (same xorshift noise, same envelopes), so numbers copied
     into a sounds.rs recipe or a soundlab sketch behave identically.
  4. python3 tools/fit_sound.py ref.wav cand.wav — compare the
     envelope rows and the band-balance table, tweak, repeat.
  5. Ship the numbers; final pass by ear in soundlab.

The candidate wav is written for A/B listening against the reference.
"""

import wave, sys
import numpy as np

SR = 22050
TAU = 2 * np.pi

# --- exact mirrors of vex_audio::synth ---
def seconds(d): return int(d * SR)

class Noise:
    def __init__(self): self.s = 0x123456789ABCDEF1
    def next(self):
        x = self.s
        x ^= (x >> 12); x &= 0xFFFFFFFFFFFFFFFF
        x ^= (x << 25) & 0xFFFFFFFFFFFFFFFF
        x ^= (x >> 27); x &= 0xFFFFFFFFFFFFFFFF
        self.s = x
        bits = np.float32(((x * 0x2545F4914F6CDD1D) & 0xFFFFFFFFFFFFFFFF) >> 40)
        return float(bits / np.float32(1 << 23) - 1.0)

def sweep(dur, f0, f1, decay, amp, shape):
    n = seconds(dur); out = np.zeros(n); ph = 0.0
    for i in range(n):
        t = i / n
        f = f0 + (f1 - f0) * t
        ph = (ph + f / SR) % 1.0
        out[i] = shape(ph) * amp * np.exp(-decay * t)
    return out

def sweep_exp(dur, f0, f1, decay, amp, shape):
    n = seconds(dur); out = np.zeros(n); ph = 0.0
    for i in range(n):
        t = i / n
        f = f0 * (f1 / f0) ** t
        ph = (ph + f / SR) % 1.0
        atk = min(i / (0.005 * SR), 1.0)
        out[i] = shape(ph) * amp * atk * np.exp(-decay * t)
    return out

def burst(dur, decay, amp):
    nz = Noise(); n = seconds(dur)
    return np.array([nz.next() * amp * np.exp(-decay * (i / n)) for i in range(n)])

def mix(a, b):
    if len(b) > len(a): a = np.pad(a, (0, len(b) - len(a)))
    out = a.copy(); out[:len(b)] += b; return out

def append(a, b): return np.concatenate([a, b])
def silence(d): return np.zeros(seconds(d))
square = lambda p: 1.0 if p < 0.5 else -1.0
saw    = lambda p: p * 2.0 - 1.0
sine   = lambda p: np.sin(p * TAU)

# --- the candidate recipe (tweak here) ---
def candidate():
    crack  = burst(0.16, 7.0, 0.55)
    sizzle = burst(0.28, 5.5, 0.10)
    zap    = sweep_exp(0.06, 880.0, 300.0, 10.0, 0.30, saw)
    ring   = append(silence(0.02), sweep_exp(0.28, 520.0, 215.0, 2.8, 0.42, sine))
    boom   = append(silence(0.12), sweep_exp(0.36, 150.0, 56.0, 3.2, 0.30, sine))
    knock  = append(silence(0.115), sweep_exp(0.08, 360.0, 190.0, 6.0, 0.38, sine))
    return mix(mix(mix(mix(mix(crack, sizzle), zap), ring), boom), knock)

# --- profile: same stats for any signal ---
def profile(x):
    hop = 110
    env = np.array([np.max(np.abs(x[i:i+hop])) for i in range(0, len(x)-hop, hop)])
    rows = []
    win, hop3 = 512, 256
    for i in range(0, len(x) - win, hop3):
        seg = x[i:i+win] * np.hanning(win)
        spec = np.abs(np.fft.rfft(seg)); freqs = np.fft.rfftfreq(win, 1/SR)
        if spec.sum() < 1e-3: continue
        p = spec / spec.sum()
        flat = np.exp(np.mean(np.log(spec + 1e-12))) / (np.mean(spec) + 1e-12)
        rows.append((i/SR*1000, freqs[np.argmax(spec)], flat,
                     p[freqs < 300].sum(), p[(freqs >= 300) & (freqs < 1500)].sum(),
                     p[freqs >= 1500].sum(), np.abs(x[i:i+win]).max()))
    return env, rows

w = wave.open(sys.argv[1], "rb")
raw = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64) / 32768.0
w.close()
ref = raw[seconds(0.036):seconds(0.505)]
ref = ref / np.abs(ref).max()

cand = candidate()
print(f"peak: cand {np.abs(cand).max():.2f} (ref normalized 1.0) · len: cand {len(cand)/SR:.2f}s ref {len(ref)/SR:.2f}s")
cn = cand / np.abs(cand).max()

er, rr = profile(ref)
ec, rc = profile(cn)
print("\n         ---- reference ----          ---- candidate ----")
print(" t(ms)   pkHz flat  lo  mid   hi  lvl | pkHz flat  lo  mid   hi  lvl")
for i in range(0, min(len(rr), len(rc)), 3):
    a, b = rr[i], rc[i]
    print(f"{a[0]:6.0f}  {a[1]:5.0f} {a[2]:.2f} {a[3]:.2f} {a[4]:.2f} {a[5]:.2f} {a[6]:.2f} |"
          f"{b[1]:5.0f} {b[2]:.2f} {b[3]:.2f} {b[4]:.2f} {b[5]:.2f} {b[6]:.2f}")
env_at = lambda e, ms: e[int(ms/5)] if int(ms/5) < len(e) else 0.0
print("\nenvelope  " + "  ".join(f"{ms}ms" for ms in [20,60,100,140,180,240,300,380]))
print("ref      " + " ".join(f"{env_at(er,ms):5.2f}" for ms in [20,60,100,140,180,240,300,380]))
print("cand     " + " ".join(f"{env_at(ec,ms):5.2f}" for ms in [20,60,100,140,180,240,300,380]))

# write candidate wav for listening
out = wave.open(sys.argv[2], "wb")
out.setnchannels(1); out.setsampwidth(2); out.setframerate(SR)
out.writeframes((np.clip(cand, -1, 1) * 32767).astype(np.int16).tobytes())
out.close()
