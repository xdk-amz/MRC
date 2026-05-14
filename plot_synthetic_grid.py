#!/usr/bin/env python3
"""Generate synthetic_mrc_grid.png — 6 workloads × 4 policies grid with 10% and 20% miss targets."""
import csv
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

policies = ['allkeys_lru', 'allkeys_lfu', 'allkeys_random', 's3_fifo']
policy_labels = {'allkeys_lru':'allkeys-lru', 'allkeys_lfu':'allkeys-lfu', 'allkeys_random':'allkeys-random', 's3_fifo':'s3-fifo'}
colors = {'allkeys_lru':'#1f77b4', 'allkeys_lfu':'#ff7f0e', 'allkeys_random':'#2ca02c', 's3_fifo':'#9467bd'}
styles = {'allkeys_lru':'-', 'allkeys_lfu':'-', 'allkeys_random':'--', 's3_fifo':'-'}

# Load all data
all_data = {}
km_pct = None
for pol in policies:
    for w in ['stable_zipfian_hot_set', 'hot_core_noisy_tail', 'rotating_hot_sets', 'stable_hot_set_plus_scans', 'uniform_random', 'sequential_scan']:
        path = f'/home/xdk/oss/MRC/out/synthetic_10m_results/{w}/{pol}_no_key_spilling_mrc_curves.csv'
        key = (w, pol)
        all_data[key] = {'cap_pct': [], 'miss': []}
        with open(path) as f:
            rdr = csv.DictReader(f)
            for r in rdr:
                total = int(r['total_bytes'])
                cap_pct = int(r['capacity_bytes']) / total * 100
                all_data[key]['cap_pct'].append(cap_pct)
                all_data[key]['miss'].append(float(r['object_miss_ratio']) * 100)
                if km_pct is None:
                    km_pct = int(r['capacity_bytes']) / total * 100  # first point = Km

workload_order = ['stable_zipfian_hot_set', 'hot_core_noisy_tail', 'rotating_hot_sets', 'stable_hot_set_plus_scans', 'uniform_random', 'sequential_scan']
titles = {'stable_zipfian_hot_set':'Zipfian Hot Set', 'hot_core_noisy_tail':'Hot Core + Noisy Tail', 'rotating_hot_sets':'Rotating Hot Sets', 'stable_hot_set_plus_scans':'Hot Set + Periodic Scans', 'uniform_random':'Uniform Random', 'sequential_scan':'Sequential Scan'}
verdicts = {'stable_zipfian_hot_set':'✓ Excellent', 'hot_core_noisy_tail':'✓ Excellent', 'rotating_hot_sets':'~ Moderate', 'stable_hot_set_plus_scans':'~ Moderate', 'uniform_random':'✗ Poor', 'sequential_scan':'✗ Poor'}

fig, axes = plt.subplots(2, 3, figsize=(16, 10), dpi=120)
axes = axes.flatten()

for idx, w in enumerate(workload_order):
    ax = axes[idx]
    for pol in policies:
        key = (w, pol)
        ax.plot(all_data[key]['cap_pct'], all_data[key]['miss'],
                color=colors[pol], linestyle=styles[pol], linewidth=1.8,
                label=policy_labels[pol])
    ax.axhline(20, color='#d62728', linestyle='--', alpha=0.6, linewidth=1.2, label='20% miss target')
    ax.axhline(10, color='#d62728', linestyle=':', alpha=0.6, linewidth=1.2, label='10% miss target')
    ax.axvline(km_pct, color='gray', linestyle=':', alpha=0.7, linewidth=1.5, label=f'Km = {km_pct:.0f}%')
    ax.set_xlim(0, 100)
    ax.set_ylim(0, 100)
    ax.set_xlabel('DRAM (% of total data)')
    ax.set_ylabel('Miss Ratio (%)')
    ax.set_title(f'{titles[w]}\n{verdicts[w]} for tiering', fontsize=11)
    ax.grid(True, linestyle='--', alpha=0.3)
    if idx == 0:
        ax.legend(fontsize=7, loc='upper right')

fig.suptitle('Data Tiering Suitability by Workload Pattern (1M keys)\n'
    'No Key Spilling, 1:2 Key:Value Ratio, 10M events, 10% & 20% miss targets (dashed/dotted)',
    fontsize=13, fontweight='bold')
fig.tight_layout()
fig.savefig('/home/xdk/oss/MRC/synthetic_mrc_grid.png', bbox_inches='tight')
print("Done")
