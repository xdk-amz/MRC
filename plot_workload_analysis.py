#!/usr/bin/env python3
"""Generate workload_analysis_mrc.png — 4 policies (no fifo), no-key-spilling, Twitter trace."""
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

total_bytes = 423789488
total_key_bytes = 88118864
km_pct = total_key_bytes / total_bytes * 100

fracs = np.linspace(0, 1, 21)
cap_bytes = total_key_bytes + fracs * (total_bytes - total_key_bytes)
cap_pct = cap_bytes / total_bytes * 100

data = {
    'allkeys-lru': [1.0,0.14092742,0.08816879,0.06285585,0.04695661,0.03615797,0.02838314,0.02239994,0.01765690,0.01378606,0.01073304,0.00832919,0.00628387,0.00465300,0.00335918,0.00227420,0.00142370,0.00080172,0.00035160,0.00009255,0.0],
    'allkeys-lfu': [1.0,0.17030990,0.11777144,0.08954550,0.07118803,0.05775691,0.04750369,0.03917449,0.03242143,0.02666317,0.02179394,0.01757257,0.01399251,0.01089405,0.00815757,0.00589923,0.00397913,0.00242567,0.00119053,0.00034528,0.0],
    'allkeys-random': [1.0,0.17181113,0.11810670,0.08960168,0.07095533,0.05770455,0.04731393,0.03920312,0.03230514,0.02658004,0.02177915,0.01746285,0.01384653,0.01084563,0.00811558,0.00587609,0.00396613,0.00236461,0.00116716,0.00035971,0.0],
    's3-fifo': [1.0,0.11953434,0.07544315,0.05548831,0.04242610,0.03329888,0.02649477,0.02105066,0.01660173,0.01286209,0.00967382,0.00720163,0.00542323,0.00403519,0.00292958,0.00203292,0.00130324,0.00078848,0.00038679,0.00009291,0.0],
}

colors = {'allkeys-lru': '#1f77b4', 'allkeys-lfu': '#ff7f0e', 'allkeys-random': '#2ca02c', 's3-fifo': '#9467bd'}
styles = {'allkeys-lru': '-', 'allkeys-lfu': '-', 'allkeys-random': '--', 's3-fifo': '-'}

fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(15, 6), dpi=120)

for ax in (ax1, ax2):
    for name, miss in data.items():
        ax.plot(cap_pct, np.array(miss)*100, color=colors[name], linestyle=styles[name], linewidth=2, label=name)
    ax.axvline(km_pct, color='gray', linestyle=':', alpha=0.7, label=f'Km = {km_pct:.1f}%')
    ax.axhline(10, color='#d62728', linestyle='--', alpha=0.5, linewidth=1, label='10% miss target')
    ax.axhline(20, color='#d62728', linestyle=':', alpha=0.5, linewidth=1, label='20% miss target')
    ax.grid(True, linestyle='--', alpha=0.3)

ax1.set_xlim(km_pct, 100)
ax1.set_ylim(0, 50)
ax1.set_xlabel('DRAM (% of total data)', fontsize=11)
ax1.set_ylabel('Object Miss Ratio (%)', fontsize=11)
ax1.set_title('Miss Ratio Curves — All Policies', fontsize=12)
ax1.legend(loc='upper right', fontsize=9)

ax2.set_xlim(km_pct, 60)
ax2.set_ylim(0, 20)
ax2.set_xlabel('DRAM (% of total data)', fontsize=11)
ax2.set_ylabel('Object Miss Ratio (%)', fontsize=11)
ax2.set_title('Zoomed: Operating Region (20–60% DRAM)', fontsize=12)
ax2.legend(loc='upper right', fontsize=9)

# Annotate crossings
for target in [10.0, 20.0]:
    for name, miss in data.items():
        miss_pct = np.array(miss)*100
        dram_at = np.interp(target, miss_pct[::-1], cap_pct[::-1])
        ax2.plot(dram_at, target, 'o', color=colors[name], markersize=5)

fig.suptitle('Twitter Cache Trace (cluster52) — Eviction Policy Comparison\n'
    '10M events, 1.6M keys, No Key Spilling (keys always in DRAM)', fontsize=13, fontweight='bold')
fig.tight_layout()
fig.savefig('/home/xdk/oss/MRC/workload_analysis_mrc.png', bbox_inches='tight')
print("Done")
