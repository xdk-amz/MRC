# MRC Simulator Design Notes

## Capacity Sweep Semantics

The simulator models two DRAM tiering strategies:

### Key-Spilling Mode
- Keys AND values compete for the same DRAM budget
- Eviction policy manages entries sized as `key_bytes + value_bytes`
- Capacity sweep: `[0, total_key_bytes + total_value_bytes]`
- At capacity=0: 100% miss (nothing cached)
- At capacity=total: 0% miss (everything cached)

### No-Key-Spilling Mode
- Keys are ALWAYS resident in DRAM (invariant)
- Only values are managed by the eviction policy
- The eviction policy capacity = `total_DRAM - total_key_bytes` (value budget only)
- Entry size for the policy = `value_bytes` only (keys don't participate in eviction)
- Capacity sweep: `[total_key_bytes, total_key_bytes + total_value_bytes]`
  - At capacity=total_key_bytes: policy_cap=0, 100% miss (keys fit, zero value budget)
  - At capacity=total: policy_cap=total_value_bytes, 0% miss (everything cached)

### Why sweep starts at total_key_bytes for no-key-spilling
Any DRAM budget below `Sum(key_bytes)` is physically impossible — you can't fit all keys.
Sweeping below this point is meaningless. The curve starts at `(Km, 100%)` where
`Km = total_key_bytes / total_bytes`.

### Plotting
Both curves share the same X-axis: "Total DRAM usage (% of total key+value bytes)".
- `capacity_bytes / total_bytes * 100` for both modes
- No-key-spilling starts at Km% on the X-axis
- The dead zone left of Km is where only key-spilling can operate

### Key Overhead (--key-overhead flag)
The trace's `key_bytes` field reports `zmalloc_size` of the SDS string allocation only.
It does NOT include:
- `dictEntry`: 24 bytes (3 pointers)
- key `robj` (redisObject): 16 bytes

Total missing overhead: 40 bytes per key (default `--key-overhead 40`).
This is added to every key_size at load time before computing totals.
Set `--key-overhead 0` to use raw trace values.
