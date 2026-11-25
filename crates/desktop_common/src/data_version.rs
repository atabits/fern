//! Scoped cross-window/cross-view data invalidation via [`DataVersion`] and [`ViewRefreshState`].

use std::ops::BitOr;

use gpui::{App, EntityId, Global};

const DOMAIN_COUNT: usize = 6;

/// Coarse data domains carried in [`DataVersion`] so views ignore edits outside their scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataScope(u16);

impl DataScope {
    pub const NONE: Self = Self(0);
    pub const TASKS: Self = Self(1 << 0);
    pub const LISTS: Self = Self(1 << 1);
    pub const HABITS: Self = Self(1 << 2);
    pub const FOCUS: Self = Self(1 << 3);
    pub const COUNTDOWN: Self = Self(1 << 4);
    pub const PREFS: Self = Self(1 << 5);
    pub const ALL: Self = Self((1 << DOMAIN_COUNT) - 1);

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl BitOr for DataScope {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

/// Counts changes made in any view or window so peer views reload only affected domains.
/// `by` identifies the emitter so it skips reloading on its own notification.
#[derive(Default)]
pub struct DataVersion {
    version: u64,
    by: Option<EntityId>,
    scopes: [u64; DOMAIN_COUNT],
}

impl Global for DataVersion {}

impl DataVersion {
    pub fn snapshot(cx: &App) -> [u64; DOMAIN_COUNT] {
        cx.try_global::<Self>().map_or([0; DOMAIN_COUNT], |d| d.scopes)
    }
}

/// Notifies peer views/windows that `by` mutated every data domain (`DataScope::ALL`).
pub fn data_changed(by: EntityId, cx: &mut App) {
    data_changed_scoped(by, DataScope::ALL, cx);
}

/// Notifies peer views/windows that `by` mutated the domains indicated by `scope`.
pub fn data_changed_scoped(by: EntityId, scope: DataScope, cx: &mut App) {
    let data = cx.default_global::<DataVersion>();
    data.version += 1;
    data.by = Some(by);
    let mask = if scope.bits() == 0 {
        DataScope::ALL.bits()
    } else {
        scope.bits()
    };
    for (idx, slot) in data.scopes.iter_mut().enumerate() {
        if (mask & (1 << idx)) != 0 {
            *slot += 1;
        }
    }
}

/// Whether the last change was made by a view other than `observer`.
pub fn changed_elsewhere(observer: EntityId, cx: &App) -> bool {
    cx.try_global::<DataVersion>()
        .is_some_and(|data| data.by != Some(observer))
}

/// Tracks active visibility, deferred dirty state, and seen per-domain versions for a workspace view.
#[derive(Clone, Debug)]
pub struct ViewRefreshState {
    active: bool,
    dirty: bool,
    scope: DataScope,
    seen_versions: [u64; DOMAIN_COUNT],
}

impl ViewRefreshState {
    pub fn new(active: bool, scope: DataScope, cx: &App) -> Self {
        Self {
            active,
            dirty: !active,
            scope,
            seen_versions: DataVersion::snapshot(cx),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Marks the view clean after loading from `Store`.
    pub fn mark_clean(&mut self, cx: &App) {
        self.dirty = false;
        self.seen_versions = DataVersion::snapshot(cx);
    }

    /// Evaluates a `DataVersion` global update for `observer`.
    /// Returns `true` if the view is active and at least one subscribed domain advanced.
    pub fn on_data_version(&mut self, observer: EntityId, cx: &App) -> bool {
        if !changed_elsewhere(observer, cx) {
            return false;
        }
        let current = DataVersion::snapshot(cx);
        let mask = self.scope.bits();
        let relevant = (0..DOMAIN_COUNT)
            .any(|idx| (mask & (1 << idx)) != 0 && current[idx] != self.seen_versions[idx]);
        if !relevant {
            return false;
        }
        if self.active {
            self.dirty = false;
            self.seen_versions = current;
            true
        } else {
            self.dirty = true;
            false
        }
    }

    /// Handles an unconditional external sync trigger (e.g. server sync or Settings close).
    /// Returns `true` if the view is currently active and should reload now; otherwise flags `dirty`.
    pub fn mark_dirty_or_reload(&mut self) -> bool {
        if self.active {
            self.dirty = false;
            true
        } else {
            self.dirty = true;
            false
        }
    }

    /// Switches active state and returns `true` when transitioning to active with pending changes.
    pub fn set_active(&mut self, active: bool, cx: &App) -> bool {
        self.active = active;
        if active && self.dirty {
            self.dirty = false;
            self.seen_versions = DataVersion::snapshot(cx);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::AppContext as _;

    use super::*;

    struct DummyView;

    #[gpui::test]
    fn scoped_notifications_filter_unrelated_domains_and_defer_inactive_views(
        cx: &mut gpui::TestAppContext,
    ) {
        let emitter_a = cx.update(|cx| cx.new(|_| DummyView)).entity_id();
        let observer_habits = cx.update(|cx| cx.new(|_| DummyView)).entity_id();
        let observer_countdown = cx.update(|cx| cx.new(|_| DummyView)).entity_id();

        cx.update(|cx| {
            let mut active_habits = ViewRefreshState::new(true, DataScope::HABITS | DataScope::PREFS, cx);
            let mut inactive_countdown =
                ViewRefreshState::new(false, DataScope::COUNTDOWN | DataScope::PREFS, cx);
            inactive_countdown.mark_clean(cx);

            // 1. Mutate TASKS -> neither HABITS nor COUNTDOWN cares.
            data_changed_scoped(emitter_a, DataScope::TASKS, cx);
            assert!(!active_habits.on_data_version(observer_habits, cx));
            assert!(!inactive_countdown.on_data_version(observer_countdown, cx));
            assert!(!inactive_countdown.is_dirty());

            // 2. Self-mutation of HABITS by observer_habits does not reload observer_habits.
            data_changed_scoped(observer_habits, DataScope::HABITS, cx);
            assert!(!active_habits.on_data_version(observer_habits, cx));

            // 3. Peer mutation of HABITS triggers active_habits immediately.
            data_changed_scoped(emitter_a, DataScope::HABITS, cx);
            assert!(active_habits.on_data_version(observer_habits, cx));
            assert!(!active_habits.on_data_version(observer_habits, cx));

            // 4. Countdown mutation followed immediately by unrelated TASKS mutation
            //    never shadows the pending COUNTDOWN reload for the inactive countdown view.
            data_changed_scoped(emitter_a, DataScope::COUNTDOWN, cx);
            assert!(!inactive_countdown.on_data_version(observer_countdown, cx));
            assert!(inactive_countdown.is_dirty());
            data_changed_scoped(emitter_a, DataScope::TASKS, cx);
            assert!(!inactive_countdown.on_data_version(observer_countdown, cx));
            assert!(inactive_countdown.set_active(true, cx));
            assert!(!inactive_countdown.is_dirty());
        });
    }
}
