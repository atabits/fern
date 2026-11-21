//! `O(N)` subtask tree index built over a slice of [`Task`]s.
//!
//! Downstream views can build a [`TaskTree`] once per reload or render pass to eliminate
//! repeated `O(N²)` scans when looking up children, computing indentation depths, counting
//! subtask progress, or flattening expanded trees.

use std::collections::{HashMap, HashSet};

use crate::model::{Id, Task};

/// `O(N)` indexed parent-child hierarchy over a borrowed slice of [`Task`]s.
///
/// Preserves the input slice's relative ordering for roots and siblings, and guards against
/// cycles in synced data so every task remains reachable without infinite recursion.
#[derive(Debug)]
pub struct TaskTree<'a> {
    tasks: &'a [Task],
    index_by_id: HashMap<Id, usize>,
    children_by_parent: HashMap<Id, Vec<usize>>,
    roots: Vec<usize>,
}

impl<'a> TaskTree<'a> {
    /// Builds a [`TaskTree`] in `O(N)` time and space.
    pub fn build(tasks: &'a [Task]) -> Self {
        let mut index_by_id = HashMap::with_capacity(tasks.len());
        for (idx, task) in tasks.iter().enumerate() {
            index_by_id.insert(task.id, idx);
        }

        let mut children_by_parent: HashMap<Id, Vec<usize>> = HashMap::new();
        let mut roots = Vec::new();
        for (idx, task) in tasks.iter().enumerate() {
            match task.parent_id {
                Some(parent_id) if parent_id != task.id && index_by_id.contains_key(&parent_id) => {
                    children_by_parent.entry(parent_id).or_default().push(idx);
                }
                _ => roots.push(idx),
            }
        }

        // Guard against disconnected cycles (e.g. A -> B -> A synced from another device):
        // mark everything reachable from `roots`, then promote one representative per cycle.
        let mut visited = vec![false; tasks.len()];
        let mut stack = roots.clone();
        while let Some(idx) = stack.pop() {
            if visited[idx] {
                continue;
            }
            visited[idx] = true;
            if let Some(children) = children_by_parent.get(&tasks[idx].id) {
                for &child_idx in children {
                    if !visited[child_idx] {
                        stack.push(child_idx);
                    }
                }
            }
        }
        for idx in 0..tasks.len() {
            if visited[idx] {
                continue;
            }
            roots.push(idx);
            let mut cycle_stack = vec![idx];
            while let Some(cur) = cycle_stack.pop() {
                if visited[cur] {
                    continue;
                }
                visited[cur] = true;
                if let Some(children) = children_by_parent.get(&tasks[cur].id) {
                    for &child_idx in children {
                        if !visited[child_idx] {
                            cycle_stack.push(child_idx);
                        }
                    }
                }
            }
        }

        Self { tasks, index_by_id, children_by_parent, roots }
    }

    /// Total number of tasks indexed in this tree.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Whether the indexed task slice is empty.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// `O(1)` lookup of a task by ID.
    pub fn get(&self, id: Id) -> Option<&'a Task> {
        self.index_by_id.get(&id).map(|&idx| &self.tasks[idx])
    }

    /// `O(1)` membership test.
    pub fn contains(&self, id: Id) -> bool {
        self.index_by_id.contains_key(&id)
    }

    /// Top-level root tasks in their original slice order (including tasks whose parent is outside
    /// this slice).
    pub fn roots(&self) -> impl Iterator<Item = &'a Task> + '_ {
        self.roots.iter().map(|&idx| &self.tasks[idx])
    }

    /// Direct children of `parent` in their original slice order (`O(1)` lookup).
    pub fn children(&self, parent: Id) -> impl Iterator<Item = &'a Task> + '_ {
        self.children_by_parent
            .get(&parent)
            .into_iter()
            .flat_map(|indices| indices.iter().map(|&idx| &self.tasks[idx]))
    }

    /// Whether `parent` has at least one direct child in this slice (`O(1)`).
    pub fn has_children(&self, parent: Id) -> bool {
        self.children_by_parent.get(&parent).is_some_and(|c| !c.is_empty())
    }

    /// Number of direct children of `parent` in this slice (`O(1)`).
    pub fn child_count(&self, parent: Id) -> usize {
        self.children_by_parent.get(&parent).map_or(0, Vec::len)
    }

    /// Nesting depth of `id` within this slice (`0` for roots), cycle-safe.
    pub fn depth(&self, id: Id) -> usize {
        let mut depth = 0usize;
        let mut seen = HashSet::from([id]);
        let mut cur = id;
        while let Some(parent_id) = self.get(cur).and_then(|t| t.parent_id) {
            if !self.index_by_id.contains_key(&parent_id) || !seen.insert(parent_id) {
                break;
            }
            depth += 1;
            cur = parent_id;
        }
        depth
    }

    /// Direct child subtask progress as `(closed, total)` (excluding deleted subtasks).
    pub fn direct_subtask_progress(&self, parent: Id) -> (usize, usize) {
        let mut closed = 0usize;
        let mut total = 0usize;
        for child in self.children(parent) {
            if child.is_deleted() {
                continue;
            }
            total += 1;
            if child.is_closed() {
                closed += 1;
            }
        }
        (closed, total)
    }

    /// Recursive descendant subtask progress as `(closed, total)` (excluding deleted tasks), cycle-safe.
    pub fn subtask_progress(&self, parent: Id) -> (usize, usize) {
        let mut closed = 0usize;
        let mut total = 0usize;
        for desc in self.descendants(parent) {
            if desc.is_deleted() {
                continue;
            }
            total += 1;
            if desc.is_closed() {
                closed += 1;
            }
        }
        (closed, total)
    }

    /// All descendants of `parent` in pre-order DFS order, cycle-safe.
    pub fn descendants(&self, parent: Id) -> Vec<&'a Task> {
        let mut out = Vec::new();
        let mut seen = HashSet::from([parent]);
        let mut stack: Vec<usize> = self
            .children_by_parent
            .get(&parent)
            .map(|kids| kids.iter().rev().copied().collect())
            .unwrap_or_default();
        while let Some(idx) = stack.pop() {
            let task = &self.tasks[idx];
            if !seen.insert(task.id) {
                continue;
            }
            out.push(task);
            if let Some(kids) = self.children_by_parent.get(&task.id) {
                for &kid_idx in kids.iter().rev() {
                    if !seen.contains(&self.tasks[kid_idx].id) {
                        stack.push(kid_idx);
                    }
                }
            }
        }
        out
    }

    /// Pre-order DFS flattening of the tree starting from [`Self::roots`], recursing into
    /// children only when `is_expanded(task.id)` returns `true`. Cycle-safe.
    pub fn flatten_with_depth(&self, is_expanded: impl Fn(Id) -> bool) -> Vec<(&'a Task, usize)> {
        let mut out = Vec::with_capacity(self.tasks.len());
        let mut seen = HashSet::with_capacity(self.tasks.len());
        let mut stack: Vec<(usize, usize)> = self.roots.iter().rev().map(|&idx| (idx, 0)).collect();
        while let Some((idx, depth)) = stack.pop() {
            let task = &self.tasks[idx];
            if !seen.insert(task.id) {
                continue;
            }
            out.push((task, depth));
            if is_expanded(task.id)
                && let Some(kids) = self.children_by_parent.get(&task.id)
            {
                for &kid_idx in kids.iter().rev() {
                    if !seen.contains(&self.tasks[kid_idx].id) {
                        stack.push((kid_idx, depth + 1));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    #[test]
    fn builds_hierarchy_computes_progress_and_flattens_with_expand_predicate() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let r1 = store.add_task("Root 1")?;
        let c1 = store.add_subtask(r1.id, "Child 1")?;
        let gc1 = store.add_subtask(c1.id, "Grandchild 1")?;
        let c2 = store.add_subtask(r1.id, "Child 2")?;
        let r2 = store.add_task("Root 2")?;
        store.set_completed(gc1.id, true)?;
        store.set_completed(c2.id, true)?;

        // Order explicitly for deterministic check: r1, c1, gc1, c2, r2.
        let ordered = vec![
            store.task(r1.id)?.unwrap(),
            store.task(c1.id)?.unwrap(),
            store.task(gc1.id)?.unwrap(),
            store.task(c2.id)?.unwrap(),
            store.task(r2.id)?.unwrap(),
        ];
        let tree = TaskTree::build(&ordered);

        assert_eq!(tree.len(), 5);
        assert!(!tree.is_empty());
        assert_eq!(tree.get(c1.id).unwrap().title, "Child 1");
        assert!(tree.has_children(r1.id));
        assert!(!tree.has_children(r2.id));
        assert_eq!(tree.child_count(r1.id), 2);
        assert_eq!(tree.depth(r1.id), 0);
        assert_eq!(tree.depth(c1.id), 1);
        assert_eq!(tree.depth(gc1.id), 2);

        let root_ids: Vec<Id> = tree.roots().map(|t| t.id).collect();
        assert_eq!(root_ids, vec![r1.id, r2.id]);
        let r1_kids: Vec<Id> = tree.children(r1.id).map(|t| t.id).collect();
        assert_eq!(r1_kids, vec![c1.id, c2.id]);

        assert_eq!(tree.direct_subtask_progress(r1.id), (1, 2));
        assert_eq!(tree.subtask_progress(r1.id), (2, 3));

        // Expand only r1 (not c1): gc1 stays collapsed.
        let flat: Vec<(Id, usize)> = tree
            .flatten_with_depth(|id| id == r1.id)
            .into_iter()
            .map(|(t, d)| (t.id, d))
            .collect();
        assert_eq!(flat, vec![(r1.id, 0), (c1.id, 1), (c2.id, 1), (r2.id, 0)]);

        // Expand all: full DFS order with depths.
        let full: Vec<(Id, usize)> = tree
            .flatten_with_depth(|_| true)
            .into_iter()
            .map(|(t, d)| (t.id, d))
            .collect();
        assert_eq!(full, vec![(r1.id, 0), (c1.id, 1), (gc1.id, 2), (c2.id, 1), (r2.id, 0)]);
        Ok(())
    }

    #[test]
    fn handles_missing_parents_and_synced_cycles_safely() -> anyhow::Result<()> {
        let store = Store::open_in_memory()?;
        let mut a = store.add_task("A")?;
        let mut b = store.add_task("B")?;
        let mut orphan = store.add_task("Orphan")?;
        a.parent_id = Some(b.id);
        b.parent_id = Some(a.id);
        orphan.parent_id = Some(uuid::Uuid::now_v7());

        let a_id = a.id;
        let tasks = vec![a, b, orphan];
        let tree = TaskTree::build(&tasks);

        // Orphan whose parent isn't in slice becomes a root; cycle members don't disappear or loop.
        let flat = tree.flatten_with_depth(|_| true);
        assert_eq!(flat.len(), 3);
        assert!(tree.depth(a_id) <= 2);
        assert_eq!(tree.descendants(a_id).len(), 1);
        Ok(())
    }
}
