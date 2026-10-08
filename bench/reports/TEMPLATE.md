# <topic>: what changed and what it cost

- date: YYYY-MM-DD
- machine: <cpu model, cores, RAM, OS>
- commit range: <base sha>..<head sha>
- tree shape: <files / depth / fanout / size / seed, or "the standard 200f-4d-4w-2048b-seed42">
- command: `just bench -- --baseline before` (or the exact invocation)

## Results

| Scenario | Case | Before | After | Change |
|:---------|:-----|:-------|:------|:-------|
| scan | | | | |
| diff | | | | |
| up | | | | |
| down | | | | |

Paste the criterion summary lines for the runs that matter under the table if the percentages hide something (outliers, change in iteration count).

## Verdict

<one line: landed / reverted, and the number that decided it>
