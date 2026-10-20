// SPDX-License-Identifier: Apache-2.0
export function representatives(executions) {
  const byCommit = new Map();
  for (const execution of executions) {
    if (
      execution.state !== 2 ||
      execution.representative === false ||
      execution.context.pullRequest > 0
    )
      continue;
    const previous = byCommit.get(execution.context.commit);
    if (!previous || execution.registrationOrder > previous.registrationOrder)
      byCommit.set(execution.context.commit, execution);
  }
  return byCommit;
}
export function points(commits, executions, dataset, column, row) {
  const reps = representatives(executions);
  return [...commits].reverse().flatMap((commit, index) => {
    const execution = reps.get(commit);
    const measurement = execution?.measurements.find(
      (item) => item.dataset.id === dataset,
    );
    const item = measurement?.rows.find((item) => item.id === row);
    if (!item || item.values[column] === undefined) return [];
    return [
      {
        commit,
        index,
        value: item.values[column],
        unit: measurement.dataset.columns.find((c) => c.id === column)?.unit,
        execution: execution.context.executionId,
      },
    ];
  });
}
export function readable(value, unit) {
  const numeric = Number(value);
  if (!Number.isFinite(numeric)) return `${value} ${unit}`;
  if (unit === "B" || unit === "B/s") {
    const power = Math.min(
      3,
      Math.max(0, Math.floor(Math.log2(Math.abs(numeric) || 1) / 10)),
    );
    return `${(numeric / 1024 ** power).toLocaleString("en", { maximumFractionDigits: 3 })} ${["B", "KiB", "MiB", "GiB"][power]}${unit.endsWith("/s") ? "/s" : ""}`;
  }
  return `${numeric.toLocaleString("en", { maximumFractionDigits: 6 })} ${unit}`;
}

// Conversion is only for graph coordinates. Exact stored values stay in tables.
export function graphValue(value, source, target) {
  const sizes = {
    B: 1,
    kB: 1e3,
    MB: 1e6,
    GB: 1e9,
    KiB: 1024,
    MiB: 1024 ** 2,
    GiB: 1024 ** 3,
  };
  const times = { ns: 1e-9, μs: 1e-6, ms: 1e-3, s: 1 };
  const unit = (name) => {
    if (name in sizes) return ["size", sizes[name]];
    if (name in times) return ["time", times[name]];
    if (name?.endsWith("/s") && name.slice(0, -2) in sizes)
      return ["byte-rate", sizes[name.slice(0, -2)]];
    if (["count", "%", "unitless", "ops/s"].includes(name)) return [name, 1];
    return [];
  };
  const [sourceKind, sourceScale] = unit(source);
  const [targetKind, targetScale] = unit(target);
  if (!sourceKind || sourceKind !== targetKind) return NaN;
  return (Number(value) * sourceScale) / targetScale;
}
