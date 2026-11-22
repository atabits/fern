import fern, { type Priority, type Task } from "fern";
import {
  Badge,
  Button,
  Card,
  Checkbox,
  Divider,
  Donut,
  EmptyState,
  Fragment,
  HStack,
  Heading,
  IconButton,
  Input,
  ListItem,
  Progress,
  Scroll,
  Section,
  SegmentedTabs,
  Select,
  Spacer,
  Svg,
  Switch,
  Text,
  VStack,
  defineView,
  useEffect,
  useState,
} from "@fern/sdk";

const PLANNER_LOGO_SVG = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="m9 12 2 2 4-4"/></svg>`;

function formatPriorityBadge(priority: Task["priority"]): string | undefined {
  if (priority === "none") return undefined;
  return priority.toUpperCase();
}

function PlannerView() {
  const defaultCompleted = Boolean(fern.settings.get<boolean>("show_completed"));
  const [tab, setTab] = useState<"open" | "done">(defaultCompleted ? "done" : "open");
  const [draftTitle, setDraftTitle] = useState("");
  const [draftRevision, setDraftRevision] = useState(1);
  const [priority, setPriority] = useState<Priority>("high");
  const [highOnly, setHighOnly] = useState(false);
  const [prefixEnabled, setPrefixEnabled] = useState(true);
  const [completedHistoryCount, setCompletedHistoryCount] = useState(0);

  useEffect(() => {
    const saved = fern.storage.get<number>("completed_count") ?? 0;
    setCompletedHistoryCount(saved);
  }, []);

  const allTasks = fern.tasks.list();
  const completedCount = allTasks.filter((t) => t.completed).length;
  const openCount = allTasks.length - completedCount;
  const completionRatio = allTasks.length > 0 ? completedCount / allTasks.length : 0;

  const visibleTasks = allTasks.filter((task) => {
    if (tab === "open" ? task.completed : !task.completed) {
      return false;
    }
    if (highOnly && task.priority !== "high") {
      return false;
    }
    return true;
  });

  function createDraftTask(rawText?: string): void {
    const trimmed = (rawText ?? draftTitle).trim();
    if (!trimmed) {
      fern.ui.toast("Enter a task title first.", "warning");
      return;
    }
    const prefix = prefixEnabled
      ? (fern.settings.get<string>("default_prefix") ?? "[Focus]").trim()
      : "";
    const title = prefix ? `${prefix} ${trimmed}` : trimmed;
    fern.tasks.create({ title, priority });
    setDraftTitle("");
    setDraftRevision((rev) => rev + 1);
    fern.ui.toast(`Added "${title}"`, "success");
  }

  return (
    <VStack gap={12} padding={16}>
      <HStack justify="between" align="center">
        <HStack gap={8} align="center">
          <Svg content={PLANNER_LOGO_SVG} width={20} height={20} />
          <Heading level={2}>Focus Planner</Heading>
          <Badge color="#2563eb">{openCount} open</Badge>
        </HStack>
        <IconButton
          icon="refresh_cw"
          tooltip="Sync history counter"
          onClick={() => {
            const latest = fern.storage.get<number>("completed_count") ?? 0;
            setCompletedHistoryCount(latest);
          }}
        />
      </HStack>

      <HStack gap={12} align="center">
        <Card title="Completion">
          <HStack gap={12} align="center">
            <Donut
              value={completionRatio}
              size={48}
              label={`${Math.round(completionRatio * 100)}%`}
            />
            <VStack gap={4}>
              <Text weight="semibold">
                {completedCount} of {allTasks.length} completed
              </Text>
              <Text muted size="sm">
                Plugin observed completions: {completedHistoryCount}
              </Text>
            </VStack>
          </HStack>
        </Card>
        <Card title="Open Progress">
          <VStack gap={8}>
            <Progress value={completionRatio} label="Overall tasks done" />
            <Switch
              key="high-only-switch"
              checked={highOnly}
              label="Only show high priority"
              onChange={(checked) => setHighOnly(checked)}
            />
          </VStack>
        </Card>
      </HStack>

      <Section title="Quick Capture" subtitle="Create tasks directly with priority and prefix">
        <HStack gap={8} align="center">
          <Input
            key="planner-draft-input"
            value={draftTitle}
            revision={draftRevision}
            placeholder="Plan next milestone..."
            onChange={(value) => setDraftTitle(value)}
            onSubmit={(value) => createDraftTask(value)}
          />
          <Select
            key="planner-priority-select"
            value={String(priority)}
            options={[
              { label: "High", value: "high" },
              { label: "Medium", value: "medium" },
              { label: "Low", value: "low" },
              { label: "None", value: "none" },
            ]}
            onChange={(value) => setPriority(value as Priority)}
          />
          <Button
            label="Add"
            icon="plus"
            variant="primary"
            onClick={() => createDraftTask()}
          />
        </HStack>
        <Spacer size={4} />
        <Checkbox
          key="prefix-checkbox"
          checked={prefixEnabled}
          label="Apply configured task prefix"
          onChange={(checked) => setPrefixEnabled(checked)}
        />
      </Section>

      <Divider />

      <HStack justify="between" align="center">
        <SegmentedTabs
          key="planner-tabs"
          value={tab}
          options={[
            { label: `Open (${openCount})`, value: "open" },
            { label: `Done (${completedCount})`, value: "done" },
          ]}
          onChange={(value) => setTab(value as "open" | "done")}
        />
      </HStack>

      {visibleTasks.length === 0 ? (
        <EmptyState
          icon="inbox"
          title="No matching tasks"
          subtitle="Create a task above or adjust the priority filter."
          actionLabel="Clear Filter"
          onAction={() => {
            setHighOnly(false);
            setTab("open");
          }}
        />
      ) : (
        <Scroll key="planner-task-scroll" maxHeight={360}>
          <VStack gap={6}>
            {visibleTasks.slice(0, 50).map((task) => (
              <Fragment key={task.id}>
                <ListItem
                  key={`item-${task.id}`}
                  title={task.title}
                  subtitle={task.dueDate ? `Due ${task.dueDate}` : undefined}
                  badge={formatPriorityBadge(task.priority)}
                  trailing={task.completed ? "Done" : "Open"}
                  onClick={() => fern.ui.openTask(task.id)}
                />
              </Fragment>
            ))}
          </VStack>
        </Scroll>
      )}
    </VStack>
  );
}

export function activate(): void {
  fern.views.register("planner", defineView(PlannerView));

  fern.commands.register("quick_plan", (arg) => {
    const prefix = fern.settings.get<string>("default_prefix") ?? "[Focus]";
    const suffix = arg && arg.trim() ? arg.trim() : "Deep work block";
    const task = fern.tasks.create({
      title: `${prefix} ${suffix}`.trim(),
      priority: "high",
    });
    fern.ui.toast(`Planned: ${task.title}`, "success");
    return task.id;
  });

  fern.events.on("task.completed", () => {
    const prev = fern.storage.get<number>("completed_count") ?? 0;
    fern.storage.set("completed_count", prev + 1);
  });
}

export function deactivate(): void {
  console.log("Focus Planner deactivated");
}
