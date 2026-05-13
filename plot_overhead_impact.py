#!/usr/bin/env python3
"""Generate key_spill_overhead_impact.png — shows how storage index overhead affects KS curves."""
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

total_bytes = 423789488
n_unique = 1615514

# No-key-spilling baseline
nks_cap = np.array([88118864,104902395,121685926,138469458,155252989,172036520,188820051,
    205603582,222387114,239170645,255954176,272737707,289521238,306304770,
    323088301,339871832,356655363,373438894,390222426,407005957,423789488], dtype=float)
nks_miss = np.array([1.0,0.14092742,0.08816879,0.06285585,0.04695661,0.03615797,
    0.02838314,0.02239994,0.01765690,0.01378606,0.01073304,0.00832919,
    0.00628387,0.00465300,0.00335918,0.00227420,0.00142370,0.00080172,
    0.00035160,0.00009255,0.0])

# Key-spilling: 0B overhead
ks0_cap = np.array([0,21189474,42378949,63568423,84757898,105947372,127136846,
    148326321,169515795,190705270,211894744,233084218,254273693,275463167,
    296652642,317842116,339031590,360221065,381410539,402600014,423789488], dtype=float)
ks0_miss = np.array([1.0,0.14108581,0.08827649,0.06302640,0.04710450,0.03633842,
    0.02849310,0.02240793,0.01771307,0.01387420,0.01081354,0.00834350,
    0.00633384,0.00465157,0.00333032,0.00228076,0.00144696,0.00081114,
    0.00036246,0.00009374,0.0])

# Key-spilling: 16B overhead
ks16_cap = np.array([25848224,45745287,65642350,85539414,105436477,125333540,145230603,
    165127666,185024730,204921793,224818856,244715919,264612982,284510046,
    304407109,324304172,344201235,364098298,383995362,403892425,423789488], dtype=float)
ks16_miss = np.array([1.0,0.14125314,0.08837584,0.06302712,0.04709329,0.03633771,
    0.02840258,0.02244192,0.01765725,0.01392655,0.01082130,0.00833361,
    0.00628578,0.00469021,0.00334177,0.00228458,0.00143420,0.00080577,
    0.00035780,0.00009708,0.0])

# Convert to %
def pct(cap): return cap / total_bytes * 100
def miss_pct(m): return m * 100

target = 8.0
def dram_at_target(cap_pct, miss_p, t):
    return np.interp(t, miss_p[::-1], cap_pct[::-1])

ks0_pct = pct(ks0_cap); ks16_pct = pct(ks16_cap); nks_pct = pct(nks_cap)
ks0_m = miss_pct(ks0_miss); ks16_m = miss_pct(ks16_miss); nks_m = miss_pct(nks_miss)

d0 = dram_at_target(ks0_pct, ks0_m, target)
d16 = dram_at_target(ks16_pct, ks16_m, target)
d_nks = dram_at_target(nks_pct, nks_m, 10.0)

fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(15, 6), dpi=120)

# Left: MRC curves
ax1.plot(nks_pct, nks_m, 'r-', linewidth=2.5, label='No Key Spilling')
ax1.plot(ks0_pct, ks0_m, 'b-', linewidth=2, label='KS: 0B overhead (ideal)')
ax1.plot(ks16_pct, ks16_m, 'b:', linewidth=2.5, label='KS: 16B overhead (realistic)')
ax1.axhline(target, color='blue', linestyle='-.', alpha=0.3, linewidth=1)
ax1.axhline(10, color='red', linestyle='-.', alpha=0.3, linewidth=1)
ax1.set_xlim(0, 50)
ax1.set_ylim(0, 20)
ax1.set_xlabel('DRAM (% of total data)', fontsize=11)
ax1.set_ylabel('Object Miss Ratio (%)', fontsize=11)
ax1.set_title('Impact of Storage Index Overhead on Key Spilling', fontsize=12)
ax1.legend(loc='upper right', fontsize=9)
ax1.grid(True, linestyle='--', alpha=0.3)

# Annotate operating points
ax1.plot(d0, target, 'o', color='blue', markersize=8)
ax1.annotate(f'0B: {d0:.1f}%', xy=(d0, target), xytext=(d0+1, target-1.5), fontsize=9, color='blue', fontweight='bold')
ax1.plot(d16, target, 'o', color='navy', markersize=8)
ax1.annotate(f'16B: {d16:.1f}%', xy=(d16, target), xytext=(d16+1, target+0.8), fontsize=9, color='navy', fontweight='bold')
ax1.plot(d_nks, 10.0, 'ro', markersize=8)
ax1.annotate(f'NKS: {d_nks:.1f}%', xy=(d_nks, 10), xytext=(d_nks+1, 11), fontsize=9, color='red', fontweight='bold')

# Right: bar chart
overheads = ['0B\n(ideal)', '16B\n(realistic)']
ks_drams = [d0, d16]
savings_vs_nks = [d_nks - d for d in ks_drams]

bars = ax2.bar(overheads, ks_drams, color=['#2196F3', '#0D47A1'], width=0.4)
ax2.axhline(d_nks, color='red', linestyle='--', linewidth=2, label=f'No Key Spilling: {d_nks:.1f}% DRAM')
ax2.set_ylabel('DRAM Required (% of total data)', fontsize=11)
ax2.set_xlabel('Per-Key Storage Index Overhead', fontsize=11)
ax2.set_title('DRAM Required at Target Miss Ratio\n(KS: 8% target, NKS: 10% target)', fontsize=12)
ax2.legend(loc='upper left', fontsize=10)
ax2.set_ylim(0, 35)
ax2.grid(True, axis='y', linestyle='--', alpha=0.3)

for bar, val, sav in zip(bars, ks_drams, savings_vs_nks):
    ax2.text(bar.get_x() + bar.get_width()/2, val + 0.5, f'{val:.1f}%\n(saves {sav:.1f}pp)',
             ha='center', fontsize=10, fontweight='bold')

fig.suptitle('Twitter Cache Trace — Storage Index Overhead Impact on Key Spilling\n'
    '10M events, 1.6M keys, allkeys-lru', fontsize=13, fontweight='bold')
fig.tight_layout()
fig.savefig('/home/xdk/oss/MRC/key_spill_overhead_impact.png', bbox_inches='tight')
print("Done")
