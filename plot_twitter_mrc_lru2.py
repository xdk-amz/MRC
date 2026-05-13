#!/usr/bin/env python3
"""Generate twitter_mrc_lru2.png — dual-panel MRC plot with 16B key-spill overhead."""
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

# Data from mrc_valkey_v2 run (allkeys-lru, trace_output_new, 16B key-spill overhead)
total_bytes = 423789488

# Key-spilling
ks_cap = np.array([25848224,45745287,65642350,85539414,105436477,125333540,145230603,
    165127666,185024730,204921793,224818856,244715919,264612982,284510046,
    304407109,324304172,344201235,364098298,383995362,403892425,423789488], dtype=float)
ks_miss = np.array([1.0,0.14125314,0.08837584,0.06302712,0.04709329,0.03633771,
    0.02840258,0.02244192,0.01765725,0.01392655,0.01082130,0.00833361,
    0.00628578,0.00469021,0.00334177,0.00228458,0.00143420,0.00080577,
    0.00035780,0.00009708,0.0])

# No-key-spilling
nks_cap = np.array([88118864,104902395,121685926,138469458,155252989,172036520,188820051,
    205603582,222387114,239170645,255954176,272737707,289521238,306304770,
    323088301,339871832,356655363,373438894,390222426,407005957,423789488], dtype=float)
nks_miss = np.array([1.0,0.14092742,0.08816879,0.06285585,0.04695661,0.03615797,
    0.02838314,0.02239994,0.01765690,0.01378606,0.01073304,0.00832919,
    0.00628387,0.00465300,0.00335918,0.00227420,0.00142370,0.00080172,
    0.00035160,0.00009255,0.0])

# Convert to % of total data
ks_pct = ks_cap / total_bytes * 100
nks_pct = nks_cap / total_bytes * 100
ks_miss_pct = ks_miss * 100
nks_miss_pct = nks_miss * 100

# Key metadata ratio
km_pct = 88118864 / total_bytes * 100  # ~20.8%

# Interpolate to find operating points
# KS target: 8% miss
ks_target = 8.0
ks_dram_at_target = np.interp(ks_target, ks_miss_pct[::-1], ks_pct[::-1])
# NKS target: 10% miss
nks_target = 10.0
nks_dram_at_target = np.interp(nks_target, nks_miss_pct[::-1], nks_pct[::-1])

fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(16, 6), dpi=120)

# === Left panel: full view ===
ax1.plot(ks_pct, ks_miss_pct, 'b-', linewidth=2.5, label='Key Spilling')
ax1.plot(nks_pct, nks_miss_pct, 'r-', linewidth=2.5, label='No Key Spilling')
ax1.axhline(ks_target, color='blue', linestyle='--', alpha=0.5, label=f'KS target ({ks_target:.0f}%)')
ax1.axhline(nks_target, color='red', linestyle='--', alpha=0.5, label=f'NKS target ({nks_target:.0f}%)')
ax1.axvline(km_pct, color='gray', linestyle=':', alpha=0.7, label=f'Km = {km_pct:.1f}%')
ax1.set_xlim(0, 100)
ax1.set_ylim(0, 50)
ax1.set_xlabel('DRAM (% of total data)', fontsize=11)
ax1.set_ylabel('Object Miss Ratio (%)', fontsize=11)
ax1.set_title('Object Miss Ratio vs DRAM', fontsize=12)
ax1.legend(loc='upper right', fontsize=9)
ax1.grid(True, linestyle='--', alpha=0.3)

# === Right panel: zoomed operating region ===
ax2.plot(ks_pct, ks_miss_pct, 'b-', linewidth=2.5, label='Key Spilling')
ax2.plot(nks_pct, nks_miss_pct, 'r-', linewidth=2.5, label='No Key Spilling')
ax2.axhline(ks_target, color='blue', linestyle='--', alpha=0.4)
ax2.axhline(nks_target, color='red', linestyle='--', alpha=0.4)
ax2.axvline(km_pct, color='gray', linestyle=':', alpha=0.7, label=f'Km = {km_pct:.1f}%')

# Annotate KS operating point
ks_miss_at_pt = np.interp(ks_dram_at_target, ks_pct, ks_miss_pct)
ax2.plot(ks_dram_at_target, ks_miss_at_pt, 'bo', markersize=10)
ax2.annotate(f'KS: {ks_dram_at_target:.0f}% DRAM\n{ks_miss_at_pt:.2f}% miss',
    xy=(ks_dram_at_target, ks_miss_at_pt), xytext=(ks_dram_at_target+2, ks_miss_at_pt+1),
    fontsize=9, color='blue', fontweight='bold')

# Annotate NKS operating point
nks_miss_at_pt = np.interp(nks_dram_at_target, nks_pct, nks_miss_pct)
ax2.plot(nks_dram_at_target, nks_miss_at_pt, 'ro', markersize=10)
ax2.annotate(f'NKS: {nks_dram_at_target:.0f}% DRAM\n{nks_miss_at_pt:.2f}% miss',
    xy=(nks_dram_at_target, nks_miss_at_pt), xytext=(nks_dram_at_target+2, nks_miss_at_pt+1),
    fontsize=9, color='red', fontweight='bold')

ax2.set_xlim(0, 50)
ax2.set_ylim(0, 20)
ax2.set_xlabel('DRAM (% of total data)', fontsize=11)
ax2.set_ylabel('Object Miss Ratio (%)', fontsize=11)
ax2.set_title('Zoomed: Operating Region', fontsize=12)
ax2.legend(loc='upper right', fontsize=9)
ax2.grid(True, linestyle='--', alpha=0.3)

fig.suptitle('Twitter Cache Trace (cluster52) — MRC Curves (allkeys-lru)\n'
    '10M events, 1.6M keys, Key:Value ≈ 1:3.8, 16B/key spill-index overhead',
    fontsize=13, fontweight='bold')
fig.tight_layout()
fig.savefig('/home/xdk/oss/MRC/twitter_mrc_lru2.png', bbox_inches='tight')
print("Saved twitter_mrc_lru2.png")
print(f"KS operating point: {ks_dram_at_target:.1f}% DRAM -> {ks_miss_at_pt:.2f}% miss")
print(f"NKS operating point: {nks_dram_at_target:.1f}% DRAM -> {nks_miss_at_pt:.2f}% miss")
