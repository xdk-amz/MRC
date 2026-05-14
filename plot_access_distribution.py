#!/usr/bin/env python3
"""Generate twitter_access_distribution.png — rank-frequency and CDF for the Twitter trace."""
import csv
from collections import Counter
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

freq = Counter()
with open('/home/xdk/data/trace1/trace_output_new.csv') as f:
    rdr = csv.reader(f)
    for row in rdr:
        freq[row[4]] += 1

sorted_freqs = np.array(sorted(freq.values(), reverse=True), dtype=float)
n = len(sorted_freqs)
total = sorted_freqs.sum()
ranks = np.arange(1, n+1)
cdf = np.cumsum(sorted_freqs) / total * 100

fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(14, 5), dpi=120)

# Left: rank-frequency (log-log)
ax1.loglog(ranks, sorted_freqs, 'b-', linewidth=1.5, alpha=0.8)
ax1.set_xlabel('Key rank (by popularity)', fontsize=11)
ax1.set_ylabel('Access count', fontsize=11)
ax1.set_title('Rank-Frequency Distribution (log-log)', fontsize=12)
ax1.grid(True, linestyle='--', alpha=0.3)
ax1.axhline(1, color='red', linestyle='--', alpha=0.5, label='1 access (one-hit-wonders)')
ax1.legend(fontsize=9)

# Right: CDF of traffic by key rank
ax2.plot(ranks/n*100, cdf, 'b-', linewidth=2)
ax2.axhline(50, color='gray', linestyle='--', alpha=0.5)
ax2.axhline(80, color='gray', linestyle='--', alpha=0.5)
ax2.axhline(90, color='gray', linestyle='--', alpha=0.5)

# Find key percentages for traffic thresholds
p50 = np.searchsorted(cdf, 50) / n * 100
p80 = np.searchsorted(cdf, 80) / n * 100
p90 = np.searchsorted(cdf, 90) / n * 100
ax2.axvline(p50, color='orange', linestyle=':', alpha=0.7)
ax2.axvline(p80, color='orange', linestyle=':', alpha=0.7)
ax2.annotate(f'{p50:.1f}% of keys → 50% traffic', xy=(p50, 50), xytext=(p50+5, 45), fontsize=9, color='darkorange')
ax2.annotate(f'{p80:.1f}% of keys → 80% traffic', xy=(p80, 80), xytext=(p80+3, 75), fontsize=9, color='darkorange')

ax2.set_xlabel('Keys (% of unique keys, ranked by popularity)', fontsize=11)
ax2.set_ylabel('Cumulative traffic (%)', fontsize=11)
ax2.set_title('Traffic Concentration (CDF)', fontsize=12)
ax2.set_xlim(0, 100)
ax2.set_ylim(0, 100)
ax2.grid(True, linestyle='--', alpha=0.3)

fig.suptitle('Twitter Cache Trace (cluster52) — Access Pattern Analysis\n'
    '10M events, 1.6M unique keys', fontsize=13, fontweight='bold')
fig.tight_layout()
fig.savefig('/home/xdk/oss/MRC/twitter_access_distribution.png', bbox_inches='tight')
print("Done")
