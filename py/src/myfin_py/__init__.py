"""myfin Python worker: data-source adapters for Baostock / AKShare / mootdx.

Data flow: fetch per task -> normalize to canonical fields (see schema.py,
mirrors crates/mf-core) -> write Parquet to staging dir -> atomic rename ->
append manifest line (JSONL). The Rust side only reads the manifest for
orchestration/validation; this package implements the worker side only.
"""

__version__ = "0.1.0"
