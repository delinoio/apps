// SPDX-License-Identifier: Apache-2.0
import { SortDirection, TaskSortKey } from "@repo/protos/lane/v1/lane_pb";
import { useEffect, useState } from "@lynx-js/react";
import {
  Button,
  Choices,
  CompactSelect,
  Field,
  MenuChoices,
  MultiChoices,
  Notice,
} from "./components";
import {
  PlanningKind,
  activeFilterCount,
  emptyFilter,
  itemsOf,
  priorityOptions,
  relationshipOptions,
  sortDirectionOptions,
  sortKeyOptions,
  toggled,
  typeOptions,
  type FilterErrors,
  type FilterState,
} from "./planning";
import type { PlanningState } from "./usePlanning";

export function TaskFilters({
  open,
  setOpen,
  filter,
  setFilter,
  errors,
  planning,
  repositoryId,
  repositoryNames,
  sortKey,
  setSortKey,
  sortDirection,
  setSortDirection,
  compact = false,
  active = true,
  separateSort = false,
}: {
  open: boolean;
  setOpen: (open: boolean) => void;
  filter: FilterState;
  setFilter: (filter: FilterState) => void;
  errors: FilterErrors;
  planning: PlanningState;
  // Empty selects the all-repository list, where owned values are qualified.
  repositoryId: string;
  repositoryNames: Map<string, string>;
  sortKey: TaskSortKey;
  setSortKey: (key: TaskSortKey) => void;
  sortDirection: SortDirection;
  setSortDirection: (direction: SortDirection) => void;
  compact?: boolean;
  active?: boolean;
  separateSort?: boolean;
}) {
  const [focusRequest, setFocusRequest] = useState(0);
  const [filterButtonId] = useState(
    () => "filters-" + Math.random().toString(36).slice(2),
  );
  const [openSortSelect, setOpenSortSelect] = useState<"key" | "direction">();
  useEffect(() => {
    "background only";
    if (focusRequest)
      lynx
        .createSelectorQuery()
        .select(`#${filterButtonId}`)
        .invoke({ method: "setFocus", params: { focus: true } })
        .exec();
  }, [filterButtonId, focusRequest]);
  const count = activeFilterCount(filter);
  const patch = (changes: Partial<FilterState>) =>
    setFilter({ ...filter, ...changes });
  const qualify = !repositoryId;
  const options = (kind: PlanningKind) =>
    itemsOf(planning.items, kind, repositoryId).map((item) => ({
      id: item.id,
      title: qualify
        ? `${repositoryNames.get(item.repositoryId) ?? "Repository"} / ${item.name}`
        : item.name,
      swatch: kind === PlanningKind.Label ? item.color : undefined,
    }));
  const unassigned = (pressed: boolean, onToggle: () => void) => ({
    title: "Unassigned",
    pressed,
    onToggle,
  });
  const empty = (noun: string, size: number) =>
    planning.status === "ready" && size === 0 ? (
      <text className="muted">
        No {noun} {qualify ? "exist yet" : "in this repository yet"}.
      </text>
    ) : null;
  const labels = options(PlanningKind.Label);
  const milestones = options(PlanningKind.Milestone);
  const cycles = options(PlanningKind.Cycle);
  return (
    <view className={`filters ${compact ? "compact-filters" : ""}`}>
      <view className="row wrap filter-actions">
        <Button
          id={filterButtonId}
          focusRequest={focusRequest}
          expanded={open}
          label={`Filters, ${count} active`}
          onClick={() => setOpen(!open)}
        >
          {count ? `Filters (${count})` : "Filters"}
        </Button>
        <Button
          disabled={count === 0}
          label="Clear filters"
          onClick={() => setFilter(emptyFilter())}
        >
          Clear filters
        </Button>
      </view>
      {open && (
        <view
          className="filter-region"
          accessibility-label="Filters"
          bindkeydown={(event) => {
            if (compact && active && event.key === "Escape") {
              setOpen(false);
              setFocusRequest((request) => request + 1);
            }
          }}
        >
          <text className="muted">
            Fields combine with AND; selected values in one field combine with
            OR. Unassigned adds tasks that have no value for that field.
          </text>
          {planning.status === "loading" && (
            <text className="muted">Loading filter options…</text>
          )}
          {planning.status === "error" && (
            <Notice>
              Could not load labels, milestones and cycles. {planning.error}{" "}
              <Button onClick={planning.retry}>Retry loading options</Button>
            </Notice>
          )}
          <MultiChoices
            label="Priority"
            selected={filter.priorities.map(String)}
            options={priorityOptions
              .filter(([value]) => value !== 0)
              .map(([value, title]) => ({ id: String(value), title }))}
            onToggle={(id) =>
              patch({ priorities: toggled(filter.priorities, Number(id)) })
            }
            extra={unassigned(filter.unassignedPriority, () =>
              patch({ unassignedPriority: !filter.unassignedPriority }),
            )}
          />
          <MultiChoices
            label="Type"
            selected={filter.types.map(String)}
            options={typeOptions
              .filter(([value]) => value !== 0)
              .map(([value, title]) => ({ id: String(value), title }))}
            onToggle={(id) =>
              patch({ types: toggled(filter.types, Number(id)) })
            }
            extra={unassigned(filter.unassignedType, () =>
              patch({ unassignedType: !filter.unassignedType }),
            )}
          />
          <MultiChoices
            label="Labels"
            selected={filter.labelIds}
            options={labels}
            onToggle={(id) => patch({ labelIds: toggled(filter.labelIds, id) })}
            extra={unassigned(filter.unlabeled, () =>
              patch({ unlabeled: !filter.unlabeled }),
            )}
          />
          {empty("labels", labels.length)}
          <view className="field-grid">
            <Field
              label="Start date from (YYYY-MM-DD)"
              value={filter.startFrom}
              onChange={(startFrom) => patch({ startFrom: startFrom.trim() })}
              error={errors.start}
            />
            <Field
              label="Start date to (YYYY-MM-DD)"
              value={filter.startTo}
              onChange={(startTo) => patch({ startTo: startTo.trim() })}
            />
          </view>
          <MultiChoices
            label="Start date"
            selected={[]}
            options={[]}
            onToggle={() => {}}
            extra={unassigned(filter.startUnassigned, () =>
              patch({ startUnassigned: !filter.startUnassigned }),
            )}
          />
          <view className="field-grid">
            <Field
              label="Due date from (YYYY-MM-DD)"
              value={filter.dueFrom}
              onChange={(dueFrom) => patch({ dueFrom: dueFrom.trim() })}
              error={errors.due}
            />
            <Field
              label="Due date to (YYYY-MM-DD)"
              value={filter.dueTo}
              onChange={(dueTo) => patch({ dueTo: dueTo.trim() })}
            />
          </view>
          <MultiChoices
            label="Due date"
            selected={[]}
            options={[]}
            onToggle={() => {}}
            extra={unassigned(filter.dueUnassigned, () =>
              patch({ dueUnassigned: !filter.dueUnassigned }),
            )}
          />
          <view className="field-grid">
            <Field
              label="Story points minimum"
              value={filter.pointsMin}
              onChange={(pointsMin) => patch({ pointsMin: pointsMin.trim() })}
              error={errors.points}
            />
            <Field
              label="Story points maximum"
              value={filter.pointsMax}
              onChange={(pointsMax) => patch({ pointsMax: pointsMax.trim() })}
            />
          </view>
          <MultiChoices
            label="Story points"
            selected={[]}
            options={[]}
            onToggle={() => {}}
            extra={unassigned(filter.pointsUnassigned, () =>
              patch({ pointsUnassigned: !filter.pointsUnassigned }),
            )}
          />
          <MultiChoices
            label="Milestone"
            selected={filter.milestoneIds}
            options={milestones}
            onToggle={(id) =>
              patch({ milestoneIds: toggled(filter.milestoneIds, id) })
            }
            extra={unassigned(filter.noMilestone, () =>
              patch({ noMilestone: !filter.noMilestone }),
            )}
          />
          {empty("milestones", milestones.length)}
          <MultiChoices
            label="Cycle"
            selected={filter.cycleIds}
            options={cycles}
            onToggle={(id) => patch({ cycleIds: toggled(filter.cycleIds, id) })}
            extra={unassigned(filter.noCycle, () =>
              patch({ noCycle: !filter.noCycle }),
            )}
          />
          {empty("cycles", cycles.length)}
          <MultiChoices
            label="Relationships"
            selected={filter.relationships.map(String)}
            options={relationshipOptions.map(([value, title]) => ({
              id: String(value),
              title,
            }))}
            onToggle={(id) =>
              patch({
                relationships: toggled(filter.relationships, Number(id)),
              })
            }
            extra={unassigned(filter.noRelationships, () =>
              patch({ noRelationships: !filter.noRelationships }),
            )}
          />
          <text className="muted">
            Unassigned in Relationships selects tasks with no relationships.
          </text>
        </view>
      )}
      {!separateSort && (
        <view className={compact ? "compact-sort-band" : "default-task-sort"}>
          {compact ? (
            <>
              <CompactSelect
                label="Sort by"
                value={sortKey}
                options={sortKeyOptions}
                onChange={setSortKey}
                active={active}
                open={openSortSelect === "key"}
                onOpenChange={(nextOpen) =>
                  setOpenSortSelect(nextOpen ? "key" : undefined)
                }
              />
              <CompactSelect
                label="Sort direction"
                value={
                  sortDirection === SortDirection.UNSPECIFIED
                    ? SortDirection.ASCENDING
                    : sortDirection
                }
                options={sortDirectionOptions}
                onChange={setSortDirection}
                disabled={sortKey === TaskSortKey.UNSPECIFIED}
                active={active}
                open={openSortSelect === "direction"}
                onOpenChange={(nextOpen) =>
                  setOpenSortSelect(nextOpen ? "direction" : undefined)
                }
              />
            </>
          ) : (
            <>
              <Choices
                exclusive
                label="Sort by"
                value={sortKey}
                options={sortKeyOptions}
                onChange={setSortKey}
              />
              <Choices
                exclusive
                label="Sort direction"
                value={
                  sortDirection === SortDirection.UNSPECIFIED
                    ? SortDirection.ASCENDING
                    : sortDirection
                }
                options={sortDirectionOptions}
                onChange={setSortDirection}
                disabled={sortKey === TaskSortKey.UNSPECIFIED}
              />
            </>
          )}
        </view>
      )}
    </view>
  );
}

export function TaskSortControls({
  sortKey,
  setSortKey,
  sortDirection,
  setSortDirection,
  compact = false,
}: {
  sortKey: TaskSortKey;
  setSortKey: (key: TaskSortKey) => void;
  sortDirection: SortDirection;
  setSortDirection: (direction: SortDirection) => void;
  compact?: boolean;
}) {
  const Control = compact ? MenuChoices : Choices;
  return (
    <view className={compact ? "proposal-sort" : "task-sort"}>
      <Control
        label="Sort by"
        value={sortKey}
        options={sortKeyOptions}
        onChange={setSortKey}
        {...(!compact ? { exclusive: true } : {})}
      />
      <Control
        label="Sort direction"
        value={
          sortDirection === SortDirection.UNSPECIFIED
            ? SortDirection.ASCENDING
            : sortDirection
        }
        options={sortDirectionOptions}
        onChange={setSortDirection}
        disabled={sortKey === TaskSortKey.UNSPECIFIED}
        {...(!compact ? { exclusive: true } : {})}
      />
      {sortKey !== TaskSortKey.UNSPECIFIED && (
        <text className="muted">
          Tasks without a value always sort last; ties sort by task ID.
        </text>
      )}
    </view>
  );
}
