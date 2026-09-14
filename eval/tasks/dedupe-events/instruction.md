Deduplicate `events.jsonl`: rows are JSON objects with an `id` field.
Keep the FIRST occurrence of each `id`, preserve order, write to `deduped.jsonl`.
