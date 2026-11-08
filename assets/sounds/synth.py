#!/usr/bin/env python3
"""Synthesises Outhouse's built-in soundboard clips ("Outhouse classics").

Every sound here is made from scratch (oscillators, noise and filters), so
there are no recordings and no third-party rights involved. Fixed seeds make
the output repeatable.

    python3 assets/sounds/synth.py            # writes web/sounds/builtin/*.mp3
    python3 assets/sounds/synth.py --wav DIR  # also keep the raw WAVs
    python3 assets/sounds/synth.py --plots DIR  # spectrograms for checking

Needs numpy and scipy, and ffmpeg (with libmp3lame) for the MP3s.
"""

import argparse
import os
import subprocess
import sys
import tempfile
import wave

import numpy as np
from scipy import signal

SR = 48_000
TARGET_LUFS = -16.0
PEAK_DBFS = -1.0
OUT_DIR = os.path.join(os.path.dirname(__file__), "..", "..", "web", "sounds", "builtin")


# ---------------------------------------------------------------- helpers

def t_axis(seconds):
    return np.arange(int(seconds * SR)) / SR


def place(buf, clip, at):
    """Adds `clip` into `buf` starting at `at` seconds (clipped to fit)."""
    i = int(at * SR)
    if i >= len(buf):
        return
    n = min(len(clip), len(buf) - i)
    buf[i:i + n] += clip[:n]


def env_adsr(n, attack, decay_tau=None, release=0.0, hold=None):
    """Attack ramp, then either an exponential decay or a hold + release."""
    t = np.arange(n) / SR
    a = np.clip(t / max(attack, 1e-4), 0, 1)
    if decay_tau is not None:
        return a * np.exp(-np.maximum(t - attack, 0) / decay_tau)
    e = a.copy()
    if hold is not None and release > 0:
        e *= np.clip(1 - (t - attack - hold) / release, 0, 1)
    return e


def bandpass(x, lo, hi, order=2):
    sos = signal.butter(order, [lo, hi], btype="bandpass", fs=SR, output="sos")
    return signal.sosfilt(sos, x)


def lowpass(x, f, order=2):
    return signal.sosfilt(signal.butter(order, f, btype="lowpass", fs=SR, output="sos"), x)


def highpass(x, f, order=2):
    return signal.sosfilt(signal.butter(order, f, btype="highpass", fs=SR, output="sos"), x)


def smooth_noise(rng, n, rate_hz):
    """Slowly wandering random curve, roughly -1..1, changing at `rate_hz`."""
    pts = max(4, int(n / SR * rate_hz) + 4)
    knots = rng.uniform(-1, 1, pts)
    x = np.linspace(0, pts - 3, n)
    return np.interp(x, np.arange(pts), knots)


def svf_bandpass(x, freq, q):
    """State-variable band-pass whose centre frequency moves per sample."""
    freq = np.broadcast_to(freq, x.shape)
    out = np.empty_like(x)
    low = band = 0.0
    damp = 1.0 / q
    for i in range(len(x)):
        f = 2.0 * np.sin(np.pi * min(freq[i], SR / 6) / SR)
        high = x[i] - low - damp * band
        band += f * high
        low += f * band
        out[i] = band
    return out


def resonator(x, freq, decay_s):
    """Two-pole resonator (a ringing mode) with a -60 dB time of `decay_s`."""
    r = np.exp(-6.9 / (decay_s * SR))
    w = 2 * np.pi * freq / SR
    b = [1 - r]
    a = [1, -2 * r * np.cos(w), r * r]
    return signal.lfilter(b, a, x)


def bubble(freq, dur, rise=0.0, tau=None, phase=0.0):
    """One bubble: a decaying sine whose pitch rises as it shrinks/surfaces.

    Real water sounds are mostly clouds of these (Minnaert resonance)."""
    n = int(dur * SR)
    t = np.arange(n) / SR
    tau = tau or dur / 4
    f = freq * (1 + rise * t / dur)
    ph = phase + 2 * np.pi * np.cumsum(f) / SR
    e = np.exp(-t / tau) * np.clip(t / 0.0015, 0, 1)
    return np.sin(ph) * e


def bubble_cloud(rng, length, rate, f_lo, f_hi, amp_curve, rise=(0.1, 0.6), dur=(0.01, 0.05)):
    """Poisson-distributed bubbles; `rate` and `amp_curve` are per-sample arrays."""
    out = np.zeros(length)
    i = 0
    while True:
        r = max(rate[min(i, length - 1)], 1e-3)
        i += int(rng.exponential(SR / r)) + 1
        if i >= length:
            break
        # Small bubbles are common, big ones rarer: log-uniform pitch.
        f = np.exp(rng.uniform(np.log(f_lo), np.log(f_hi)))
        d = rng.uniform(*dur) * (600 / f) ** 0.5
        b = bubble(f, d, rise=rng.uniform(*rise), phase=rng.uniform(0, 6.28))
        place(out, b * rng.uniform(0.3, 1.0) * amp_curve[i], i / SR)
    return out


# ------------------------------------------------- loudness (ITU BS.1770)

def k_weight(x):
    # 48 kHz coefficients straight from BS.1770.
    b1 = [1.53512485958697, -2.69169618940638, 1.19839281085285]
    a1 = [1.0, -1.69065929318241, 0.73248077421585]
    b2 = [1.0, -2.0, 1.0]
    a2 = [1.0, -1.99004745483398, 0.99007225036621]
    return signal.lfilter(b2, a2, signal.lfilter(b1, a1, x))


def lufs(x):
    y = k_weight(x)
    block, hop = int(0.4 * SR), int(0.1 * SR)
    if len(y) < block:
        y = np.pad(y, (0, block - len(y)))
    powers = np.array([np.mean(y[i:i + block] ** 2) for i in range(0, len(y) - block + 1, hop)])
    loud = -0.691 + 10 * np.log10(powers + 1e-12)
    gated = powers[loud > -70]
    if not len(gated):
        return -70.0
    rel = -0.691 + 10 * np.log10(np.mean(gated)) - 10
    gated = powers[(loud > -70) & (loud > rel)]
    return -0.691 + 10 * np.log10(np.mean(gated))


def limit(x, ceiling):
    """Look-ahead peak limiter: fast attack, 80 ms release, no clipping."""
    look = int(0.003 * SR)
    peak = np.abs(x)
    # Max over the look-ahead window so the gain is down before the peak.
    win = np.lib.stride_tricks.sliding_window_view(np.pad(peak, (0, look)), look + 1).max(axis=1)
    want = np.minimum(1.0, ceiling / np.maximum(win, 1e-9))
    g = np.empty_like(want)
    cur = 1.0
    rel = np.exp(-1 / (0.08 * SR))
    for i, w in enumerate(want):
        cur = w if w < cur else w + (cur - w) * rel
        g[i] = cur
    return x * g


def finish(x, fade_in=0.004, fade_out=0.03):
    x = x - np.mean(x)
    x = highpass(x, 30)
    n_in, n_out = int(fade_in * SR), int(fade_out * SR)
    x[:n_in] *= np.linspace(0, 1, n_in) ** 2
    x[-n_out:] *= np.linspace(1, 0, n_out) ** 2
    ceiling = 10 ** (PEAK_DBFS / 20)
    for _ in range(4):  # gain up, limit, re-measure
        x = x * 10 ** ((TARGET_LUFS - lufs(x)) / 20)
        x = limit(x, ceiling * 0.97)
    return np.clip(x, -ceiling, ceiling)


# ---------------------------------------------------------------- sounds

def plop():
    """💩 A fat water drop: impact thump, splash, a rising "bloop", ripples."""
    rng = np.random.default_rng(31)
    n = int(0.55 * SR)
    out = np.zeros(n)
    # The thing hitting the water: a short low thump.
    t = t_axis(0.08)
    thump = np.sin(2 * np.pi * (95 * t + 120 * t * (1 - t / 0.16))) * np.exp(-t / 0.018)
    place(out, 0.5 * thump, 0.0)
    # Splash: bright noise for a few ms.
    splash = bandpass(rng.standard_normal(int(0.05 * SR)), 1200, 7000) * env_adsr(int(0.05 * SR), 0.001, 0.009)
    place(out, 0.35 * splash, 0.002)
    # The cavity closing: a big bubble that swoops up in pitch, the classic bloop.
    t = t_axis(0.22)
    f = 210 + 520 * (1 - np.exp(-t / 0.045))
    ph = 2 * np.pi * np.cumsum(f) / SR
    bloop = np.sin(ph) * np.clip(t / 0.004, 0, 1) * np.exp(-t / 0.06)
    bloop += 0.12 * np.sin(2 * ph) * np.clip(t / 0.004, 0, 1) * np.exp(-t / 0.03)
    place(out, 1.0 * bloop, 0.012)
    # A couple of droplets falling back in.
    place(out, 0.28 * bubble(1150, 0.06, rise=0.5, tau=0.015), 0.16)
    place(out, 0.18 * bubble(1650, 0.05, rise=0.4, tau=0.012), 0.235)
    place(out, 0.12 * bubble(950, 0.07, rise=0.6, tau=0.02), 0.30)
    # Faint porcelain ring from the bowl.
    ring = resonator(splash, 880, 0.35) + 0.6 * resonator(splash, 1430, 0.25)
    place(out, 0.4 * ring / (np.max(np.abs(ring)) + 1e-9), 0.002)
    return finish(out, fade_out=0.06)


def flush():
    """🚽 Lever clunk, swirling rush of water, glugs down the drain, refill hiss."""
    rng = np.random.default_rng(7)
    total = 3.9
    n = int(total * SR)
    out = np.zeros(n)

    # 1. The handle: a plastic click, then the clunk of the flapper lifting.
    click = highpass(rng.standard_normal(int(0.01 * SR)), 2500) * env_adsr(int(0.01 * SR), 0.0005, 0.0015)
    place(out, 0.5 * click, 0.0)
    knock = rng.standard_normal(int(0.004 * SR)) * np.hanning(int(0.004 * SR))
    clunk = resonator(knock, 160, 0.12) + 0.7 * resonator(knock, 410, 0.08) + 0.3 * resonator(knock, 1020, 0.05)
    clunk = np.pad(clunk, (0, int(0.15 * SR)))[: int(0.15 * SR)]
    place(out, 1.2 * clunk / np.max(np.abs(clunk)), 0.07)
    click2 = bandpass(rng.standard_normal(int(0.006 * SR)), 1500, 5000) * env_adsr(int(0.006 * SR), 0.0003, 0.001)
    place(out, 0.35 * click2, 0.12)

    # 2. The rush: water-shaped noise whose colour swirls round and round.
    start, end = 0.16, 3.0
    m = int((end - start) * SR)
    tt = np.arange(m) / SR
    swell = np.clip(tt / 0.35, 0, 1) ** 1.5 * np.clip((end - start - tt) / 1.4, 0, 1) ** 1.2
    # The vortex speeds up as the bowl empties.
    swirl_rate = 1.6 + 3.2 * tt / (end - start)
    swirl = np.sin(2 * np.pi * np.cumsum(swirl_rate) / SR)
    wobble = smooth_noise(rng, m, 6)
    centre = 900 * 2 ** (0.9 * swirl + 0.3 * wobble)
    noise = rng.standard_normal(m)
    swirly = svf_bandpass(noise, centre, 2.2)
    body = lowpass(rng.standard_normal(m), 380, 4)  # the low roar of a lot of water
    hiss = bandpass(rng.standard_normal(m), 3000, 9000)  # spray
    rush = (0.55 * swirly / np.std(swirly) + 0.5 * body / np.std(body) + 0.12 * hiss / np.std(hiss))
    rush *= swell * (1 + 0.25 * swirl)
    # Lots of bubbles on top: this is what makes noise sound wet.
    rate = 40 + 520 * swell
    cloud = bubble_cloud(rng, m, rate, 250, 2200, swell, rise=(0.2, 1.2), dur=(0.015, 0.05))
    place(out, 0.28 * rush + 0.55 * cloud, start)

    # 3. The drain gulping: big low bubbles in a glug-glug rhythm.
    g0 = 2.15
    for k, (f, a) in enumerate([(240, 1.0), (205, 0.9), (275, 0.85), (190, 0.8), (230, 0.6), (180, 0.45)]):
        at = g0 + k * 0.125 + rng.uniform(-0.012, 0.012)
        glug = bubble(f, 0.16, rise=0.9, tau=0.05) + 0.4 * np.pad(bubble(f * 2.03, 0.08, rise=0.7, tau=0.02), (0, int(0.08 * SR)))
        burst = lowpass(rng.standard_normal(int(0.07 * SR)), 900) * env_adsr(int(0.07 * SR), 0.003, 0.02)
        place(out, a * 0.75 * glug, at)
        place(out, a * 0.05 * burst / np.std(burst), at)
    # The last slurp of air: a long falling-then-rising gurgle.
    t = t_axis(0.35)
    f = 150 + 140 * np.sin(np.pi * t / 0.35) ** 2 + 30 * np.sin(2 * np.pi * 23 * t)
    slurp = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.sin(np.pi * t / 0.35) ** 1.5
    place(out, 0.45 * slurp, 2.85)

    # 4. The cistern refilling: a hissing valve with a faint whistle, plus trickle.
    start = 2.55
    m = int((total - start) * SR)
    tt = np.arange(m) / SR
    fill = np.clip(tt / 0.4, 0, 1) * np.clip((total - start - tt) / 0.9, 0, 1)
    valve = bandpass(rng.standard_normal(m), 2400, 6500, 3)
    valve = valve / np.std(valve) * (1 + 0.15 * smooth_noise(rng, m, 25))
    whistle = np.sin(2 * np.pi * np.cumsum(2730 + 40 * smooth_noise(rng, m, 3)) / SR)
    trickle = bubble_cloud(rng, m, np.full(m, 60.0), 900, 2600, np.ones(m), dur=(0.01, 0.03))
    place(out, fill * (0.09 * valve + 0.02 * whistle) + 0.35 * trickle * fill, start)

    return finish(out, fade_out=0.25)


def _voice(f0, formants, tilt, amp, breath, rng, nasal=False):
    """Additive formant voice: harmonics of a glottal source shaped per block by
    a vocal-tract response (formant resonances), plus breath noise.

    f0, tilt, amp, breath are per-sample curves; formants is a list of
    (freq, bandwidth, gain) per-sample curve triples."""
    n = len(f0)
    phase = 2 * np.pi * np.cumsum(f0) / SR
    out = np.zeros(n)
    block = 240
    nharm = 60
    for b0 in range(0, n, block):
        b1 = min(n, b0 + block)
        mid = (b0 + b1) // 2
        k = np.arange(1, nharm + 1)
        fk = k * f0[mid]
        # Glottal source: ~-12 dB/octave, less when pushing hard ("pressed" voice).
        src = k ** (-tilt[mid])
        h = np.zeros(nharm)
        for (ff, bw, g) in formants:
            F, B = ff[mid], bw[mid]
            h += g[mid] / np.sqrt(1 + ((fk - F) / (B / 2)) ** 2)  # resonance peak
        if nasal:
            # Nasal anti-resonance around 1 kHz: what makes "mmm" sound closed.
            h *= 1 - 0.85 / np.sqrt(1 + ((fk - 1050) / 140) ** 2)
        a = src * h * (fk < SR / 2 - 2000)
        seg = phase[b0:b1]
        out[b0:b1] = (a[:, None] * np.sin(np.outer(k, seg))).sum(axis=0)
    out = out / (np.max(np.abs(out)) + 1e-9)
    # Breath: noise through the same formants.
    nz = rng.standard_normal(n)
    breathy = np.zeros(n)
    for (ff, bw, g) in formants:
        breathy += np.mean(g) * svf_bandpass(nz, ff, max(float(np.mean(ff / bw)), 1.0))
    breathy /= np.std(breathy) + 1e-9
    return amp * ((1 - breath) * out + 0.25 * breath * breathy)


def strain():
    """😣 A cartoon "mmmnnnNNNGH... nngh! ... haaah" of effort, then relief."""
    rng = np.random.default_rng(1234)

    def curve(points, n):
        xs, ys = zip(*points)
        return np.interp(np.linspace(0, xs[-1], n), xs, ys)

    # Part 1: closed-mouth hum, rising in pitch and tension, getting shaky.
    d1 = 1.45
    n = int(d1 * SR)
    t = np.arange(n) / SR
    f0 = curve([(0, 112), (0.5, 120), (1.0, 138), (1.3, 158), (1.45, 166)], n)
    shake_depth = curve([(0, 0.004), (0.6, 0.012), (1.45, 0.035)], n)
    shake = np.sin(2 * np.pi * np.cumsum(curve([(0, 5.5), (1.45, 8.5)], n)) / SR)
    jitter = smooth_noise(rng, n, 40) * 0.012
    f0 = f0 * (1 + shake_depth * shake + jitter)
    amp = curve([(0, 0), (0.06, 0.55), (0.5, 0.7), (1.2, 0.95), (1.4, 1.0), (1.45, 0)], n)
    amp *= 1 + 0.6 * shake_depth / 0.035 * 0.25 * shake  # shimmer with the shake
    tilt = curve([(0, 2.0), (0.8, 1.6), (1.45, 1.15)], n)  # brighter = more strained
    # "mmm" moving to "nnn" then to a squeezed "ng": F2/F3 creep up.
    F = [
        (curve([(0, 260), (1.45, 300)], n), np.full(n, 90.0), np.full(n, 1.0)),
        (curve([(0, 1200), (0.7, 1500), (1.45, 1900)], n), np.full(n, 200.0), curve([(0, 0.18), (1.45, 0.35)], n)),
        (curve([(0, 2500), (1.45, 2700)], n), np.full(n, 260.0), curve([(0, 0.06), (1.45, 0.2)], n)),
    ]
    hum = _voice(f0, F, tilt, amp, curve([(0, 0.05), (1.45, 0.1)], n), rng, nasal=True)

    # Part 2: a short extra push, "nngh!", higher.
    d2 = 0.32
    n2 = int(d2 * SR)
    f0b = curve([(0, 170), (0.15, 188), (0.32, 176)], n2) * (1 + 0.03 * np.sin(2 * np.pi * 9 * np.arange(n2) / SR) + smooth_noise(rng, n2, 40) * 0.015)
    ampb = curve([(0, 0), (0.03, 1.0), (0.25, 0.9), (0.32, 0)], n2)
    Fb = [
        (np.full(n2, 310.0), np.full(n2, 100.0), np.full(n2, 1.0)),
        (np.full(n2, 2000.0), np.full(n2, 220.0), np.full(n2, 0.4)),
        (np.full(n2, 2800.0), np.full(n2, 280.0), np.full(n2, 0.22)),
    ]
    push = _voice(f0b, Fb, np.full(n2, 1.1), ampb, np.full(n2, 0.1), rng, nasal=True)

    # Part 3: the relieved sigh, "haaaah": breathy open vowel, pitch falling.
    d3 = 0.95
    n3 = int(d3 * SR)
    f0c = curve([(0, 175), (0.2, 150), (0.95, 92)], n3) * (1 + smooth_noise(rng, n3, 30) * 0.01)
    ampc = curve([(0, 0), (0.07, 0.85), (0.35, 0.7), (0.95, 0)], n3)
    breath = curve([(0, 0.85), (0.12, 0.45), (0.5, 0.55), (0.95, 0.9)], n3)
    Fc = [
        (curve([(0, 650), (0.95, 700)], n3), np.full(n3, 110.0), np.full(n3, 1.0)),
        (curve([(0, 1150), (0.95, 1100)], n3), np.full(n3, 130.0), np.full(n3, 0.55)),
        (np.full(n3, 2600.0), np.full(n3, 200.0), np.full(n3, 0.18)),
    ]
    sigh = _voice(f0c, Fc, np.full(n3, 2.1), ampc, breath, rng)

    total = d1 + 0.12 + d2 + 0.34 + d3
    out = np.zeros(int(total * SR))
    place(out, hum, 0.0)
    place(out, push, d1 + 0.12)
    place(out, 0.9 * sigh, d1 + 0.12 + d2 + 0.34)
    return finish(out, fade_out=0.08)


def door():
    """🚪 An old wooden door: rusty hinge creaking open, then a soft knock."""
    rng = np.random.default_rng(99)
    d_creak = 1.75
    n = int(d_creak * SR)
    t = np.arange(n) / SR
    # Stick-slip friction: the hinge sticks, then slips with a click, again and
    # again. The click rate is the creak's pitch, and it wanders as the door's
    # speed changes: slow ticks, a long "eeeerrrk", a hiccup, slowing again.
    rate = np.interp(t, [0, 0.18, 0.35, 0.7, 0.9, 1.0, 1.1, 1.4, 1.75],
                     [18, 45, 160, 240, 210, 120, 260, 190, 60])
    rate = rate * (1 + 0.12 * smooth_noise(rng, n, 9))
    loud = np.interp(t, [0, 0.2, 0.4, 1.0, 1.05, 1.15, 1.5, 1.75], [0.25, 0.5, 0.9, 1.0, 0.55, 1.0, 0.8, 0.2])
    pulses = np.zeros(n)
    i = 0
    while True:
        r = rate[min(i, n - 1)]
        step = SR / r * (1 + rng.normal(0, 0.06))  # irregular, it's friction
        i += max(1, int(step))
        if i >= n:
            break
        pulses[i] += loud[i] * rng.uniform(0.6, 1.0)
    # Each slip is a short sharp kick, not a single sample.
    kick = np.exp(-np.arange(int(0.0012 * SR)) / (0.0003 * SR))
    exc = signal.fftconvolve(pulses, kick)[:n]
    # Wooden panel modes (warm, fairly damped) and the iron hinge (bright, ringy).
    wood = sum(g * resonator(exc, f, dcy) for f, g, dcy in
               [(190, 1.0, 0.09), (340, 0.9, 0.08), (560, 0.8, 0.06), (870, 0.6, 0.05), (1320, 0.45, 0.04), (2050, 0.3, 0.03)])
    hinge = sum(g * resonator(exc, f, dcy) for f, g, dcy in
                [(2900, 0.5, 0.014), (3700, 0.35, 0.011), (5200, 0.2, 0.008)])
    creak = wood / np.std(wood) + 0.3 * hinge / np.std(hinge)
    creak *= np.clip(t / 0.05, 0, 1)

    # The door swings against the wall: a soft wooden knock and a small rattle.
    def knock(strength, length=0.35):
        m = int(length * SR)
        imp = np.zeros(m)
        imp[:int(0.003 * SR)] = np.hanning(int(0.003 * SR))
        body = (1.0 * resonator(imp, 95, 0.18) + 0.9 * resonator(imp, 210, 0.12) + 0.7 * resonator(imp, 470, 0.07)
                + 0.4 * resonator(imp, 820, 0.05) + 0.2 * resonator(imp, 1600, 0.03))
        tap = bandpass(rng.standard_normal(m), 600, 3500) * env_adsr(m, 0.0005, 0.004)
        return strength * (body / np.max(np.abs(body)) + 0.25 * tap)

    out = np.zeros(int((d_creak + 0.55) * SR))
    place(out, 0.55 * creak / np.max(np.abs(creak)), 0.0)
    place(out, 1.0 * knock(1.0), d_creak - 0.04)
    place(out, 0.3 * knock(1.0, 0.2), d_creak + 0.07)
    return finish(out, fade_out=0.12)


def blowie():
    """🪰 A blowfly doing laps of the dunny: a buzzy sawtooth with wingbeat
    wobble, getting near and far (pitch shifts like a Doppler fly-by)."""
    rng = np.random.default_rng(5)
    d = 2.6
    n = int(d * SR)
    t = np.arange(n) / SR
    # Distance: it buzzes past twice. Closer = louder and a touch brighter.
    near = 0.35 + 0.65 * (np.exp(-((t - 0.75) / 0.28) ** 2) + 0.85 * np.exp(-((t - 1.85) / 0.32) ** 2))
    approach = np.gradient(near) * SR  # >0 coming closer: pitch up
    f0 = 190 * (1 + 0.035 * np.tanh(approach / 3)) * (1 + 0.03 * smooth_noise(rng, n, 4))
    ph = 2 * np.pi * np.cumsum(f0) / SR
    k = np.arange(1, 40)
    saw = np.zeros(n)
    for kk in k:
        saw += np.sin(kk * ph) / kk * (kk * 190 < 16000)
    # Wing-beat wobble and a nasal body resonance.
    saw *= 1 + 0.35 * np.sin(ph / 2 + smooth_noise(rng, n, 3))
    buzz = saw + 0.8 * resonator(saw, 1150, 0.02) + 0.4 * resonator(saw, 2600, 0.01)
    buzz = buzz / np.std(buzz)
    tone = lowpass(buzz, 2500) * (1 - near) + buzz * near
    out = tone * near * env_adsr(n, 0.15, hold=d - 0.55, release=0.4)
    # It lands, and stops dead (with a tiny tick).
    return finish(out, fade_out=0.15)


SOUNDS = {
    "flush": flush,
    "strain": strain,
    "plop": plop,
    "door": door,
    "blowie": blowie,
}


# ---------------------------------------------------------------- output

def write_wav(path, x):
    pcm = (np.clip(x, -1, 1) * 32767).astype("<i2")
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm.tobytes())


def encode_mp3(wav, mp3):
    # Bit-exact output: no encoder tag with a version string, fixed bitrate.
    subprocess.run(
        ["ffmpeg", "-v", "error", "-y", "-i", wav, "-map_metadata", "-1", "-fflags", "+bitexact",
         "-flags:a", "+bitexact", "-c:a", "libmp3lame", "-b:a", "96k", "-ac", "1", mp3],
        check=True,
    )


def plot(path, x, title):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    fig, (a1, a2) = plt.subplots(2, 1, figsize=(10, 6), sharex=True)
    t = np.arange(len(x)) / SR
    a1.plot(t, x, lw=0.4)
    a1.set_ylim(-1, 1)
    a1.set_title(title)
    a2.specgram(x, NFFT=1024, Fs=SR, noverlap=768, cmap="magma", vmin=-120)
    a2.set_ylim(0, 8000)
    fig.savefig(path, dpi=80)
    plt.close(fig)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--wav", help="also write WAVs here")
    ap.add_argument("--plots", help="write spectrogram PNGs here")
    ap.add_argument("--only", help="comma-separated names")
    args = ap.parse_args()
    os.makedirs(OUT_DIR, exist_ok=True)
    names = args.only.split(",") if args.only else list(SOUNDS)
    for name in names:
        x = SOUNDS[name]()
        peak = 20 * np.log10(np.max(np.abs(x)) + 1e-12)
        rms = 20 * np.log10(np.sqrt(np.mean(x ** 2)) + 1e-12)
        print(f"{name:8s} {len(x) / SR:5.2f} s  {lufs(x):6.1f} LUFS  peak {peak:5.1f} dBFS  rms {rms:5.1f} dBFS")
        with tempfile.TemporaryDirectory() as tmp:
            wav = os.path.join(args.wav or tmp, f"{name}.wav")
            if args.wav:
                os.makedirs(args.wav, exist_ok=True)
            write_wav(wav, x)
            encode_mp3(wav, os.path.join(OUT_DIR, f"{name}.mp3"))
        if args.plots:
            os.makedirs(args.plots, exist_ok=True)
            plot(os.path.join(args.plots, f"{name}.png"), x, name)
    return 0


if __name__ == "__main__":
    sys.exit(main())
