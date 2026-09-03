# Multi-agent scientific brainstorm panel

For a nontrivial research topic, convene a panel of subagents that model real scientists and referee it through fixed rounds. Do not spawn a panel for a quick judgment or a narrow factual question.

Roles:

- panelist: one real-scientist persona each; builds a dossier from public sources, then proposes, critiques, and ranks directions;
- prior_art_scout: checks every surviving direction against the literature and returns the closest prior work with a verdict;
- devils_advocate: attacks the panel's leading directions and the chair's summary in the final round;
- panel_rapporteur: clusters and anonymizes a round's proposals when the panel is large.

Protocol:

1. Recruit. Write the topic brief and rubric to `panel/brief.md`. Choose four to eight real scientists by the field's size and its distinct schools of thought: four for a niche, up to eight for a crowded field, spanning theory and experiment or computation, senior and rising, mainstream and dissenting; record the reasons in `memory.md`. Spawn one `panelist` per scientist with `fork_turns` set to `none`, a `task_name` equal to the scientist's slug, and a message naming the scientist, the brief path, the dossier path `panel/personas/<slug>.md`, and the round-1 output path. Wait for all of them.
2. Blind proposals. Each panelist writes `panel/round-1/<slug>.md` in the fixed template from its instructions. Panelists must not see each other's files yet.
3. Summarize. Cluster overlapping directions, strip authorship, and write `panel/summary-round-1.md`; use `panel_rapporteur` when there are more than five panelists. Send every panelist the summary path with the round-2 instruction and output path, and spawn the `prior_art_scout` on the surviving directions.
4. Critique. Each panelist attacks the other directions, steelmans the strongest opposing one, and revises or withdraws its own. Merge with the scout's verdicts; prune what is published, textbook, unfalsifiable, or without a feasible first experiment, recording each reason in the direction ledger.
5. Rank. Send the pruned list; each panelist scores every direction on the rubric with one line per criterion and names the one it would stake a year on. Spawn the `devils_advocate` on the emerging top three. Keep the top directions when their order is stable across two rounds, or after the fourth round at most.
6. Hand off. Write `strategy.md`, update `memory.md`, and close the panelists.

Give every message a narrow question, the file paths to read and write, and the return format. Ask for file paths and a three-line summary in replies, never whole documents, so the transcript stays small. Never inject your own preferred direction into a brief or summary. Treat consensus as a signal to recheck, not as a conclusion.
