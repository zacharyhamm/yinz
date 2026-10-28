//! Stack-list membership, hidden panes, serialization geometry, and headers.
use crate::plugins::PluginInstruction;
use unicode_width::UnicodeWidthStr;

use super::{StackList, StackListId, StackListRemoval, SuppressedPane, Tab};
use crate::output::CharacterChunk;
use crate::output::Output;
use crate::panes::PaneId;
use crate::panes::EMPTY_TERMINAL_CHARACTER;
use crate::ui::pane_contents_and_ui::PaneContentsAndUi;
use crate::ClientId;
use std::collections::HashMap;
use std::collections::HashSet;
use yinz_panes::pane::Pane;
use yinz_panes::resize_pty;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::InputMode;
use yinz_types::pane_size::Dimension;
use yinz_types::pane_size::Offset;
use yinz_types::pane_size::PaneGeom;
use yinz_types::position::Position;

impl Tab {
    pub fn stacked_pane_list_is_active(&self) -> bool {
        *self.stacked_pane_list.borrow()
    }
    pub fn pane_is_stack_list_member(&self, pane_id: &PaneId) -> bool {
        self.stack_list_of_member.contains_key(pane_id)
    }
    pub fn has_stack_lists(&self) -> bool {
        !self.stack_lists.is_empty()
    }
    pub(super) fn stack_list_id_of_member(&self, pane_id: &PaneId) -> Option<StackListId> {
        self.stack_list_of_member.get(pane_id).copied()
    }
    pub fn pane_is_hidden_stack_list_member(&self, pane_id: &PaneId) -> bool {
        self.stack_list_id_of_member(pane_id)
            .and_then(|stack_list_id| self.stack_lists.get(&stack_list_id))
            .map(|list| list.visible != *pane_id)
            .unwrap_or(false)
    }
    pub(super) fn stack_list_promote_target(&self, pane_id: PaneId) -> Option<PaneId> {
        let list = self
            .stack_lists
            .get(&self.stack_list_id_of_member(&pane_id)?)?;
        if list.visible != pane_id {
            return None;
        }
        let rank = list.rank_of(&pane_id).unwrap_or(0);
        list.members
            .get(rank + 1)
            .copied()
            .or_else(|| list.members[..rank].last().copied())
    }
    pub fn focus_hidden_stack_list_member(&mut self, pane_id: PaneId, client_id: ClientId) -> bool {
        self.swap_in_hidden_stack_list_member(pane_id, Some(client_id))
    }
    pub(super) fn swap_in_hidden_stack_list_member(
        &mut self,
        pane_id: PaneId,
        client_id: Option<ClientId>,
    ) -> bool {
        if !self.pane_is_hidden_stack_list_member(&pane_id) {
            return false;
        }
        match self.stack_list_id_of_member(&pane_id) {
            Some(stack_list_id) => {
                self.select_stack_list_member(stack_list_id, pane_id, client_id);
                true
            },
            None => false,
        }
    }
    pub(super) fn floating_stack_headers_visible(&self, list: &StackList) -> bool {
        self.floating_panes.fullscreen_pane_id().is_none()
            && self
                .floating_panes
                .get_pane(list.visible)
                .map(|pane| {
                    let bottom_frame = usize::from(!pane.borderless());
                    pane.rows() > list.members.len() + 1 + bottom_frame
                })
                .unwrap_or(false)
    }
    pub(super) fn stack_list_member_at_point(&self, pane_id: PaneId, point: &Position) -> PaneId {
        let Some(stack_list_id) = self.stack_list_id_of_member(&pane_id) else {
            return pane_id;
        };
        let Some(list) = self.stack_lists.get(&stack_list_id) else {
            return pane_id;
        };
        if self.floating_panes.panes_contain(&list.visible)
            && !self.floating_stack_headers_visible(list)
        {
            return list.visible;
        }
        let Some(pane) = self
            .tiled_panes
            .get_pane(pane_id)
            .or_else(|| self.floating_panes.get_pane(pane_id))
        else {
            return pane_id;
        };
        let geom = pane.current_geom();
        if point.line() < geom.y as isize {
            return pane_id;
        }
        let rank = (point.line() - geom.y as isize) as usize;
        list.members.get(rank).copied().unwrap_or(pane_id)
    }
    pub fn suppressed_stack_list_members(&self) -> impl Iterator<Item = (&PaneId, &Box<dyn Pane>)> {
        self.suppressed_panes
            .iter()
            .filter(|(pane_id, _)| self.stack_list_of_member.contains_key(pane_id))
            .map(|(pane_id, suppressed)| (pane_id, &suppressed.pane))
    }
    pub fn stack_list_serialization_geoms(&self) -> HashMap<PaneId, PaneGeom> {
        let mut synthetic_geoms = HashMap::new();
        if self.stack_lists.is_empty() {
            return synthetic_geoms;
        }
        let mut next_synthetic_stack_id = self
            .tiled_panes
            .get_panes()
            .filter_map(|(_, pane)| pane.position_and_size().stacked)
            .max()
            .map(|highest_stack_id| highest_stack_id + 1)
            .unwrap_or(0);
        for list in self.stack_lists.values() {
            let Some(visible_pane) = self.tiled_panes.get_pane(list.visible) else {
                continue;
            };
            let full_rect = visible_pane.position_and_size();
            let collapsed_member_count = list.members.len().saturating_sub(1);
            let mut running_y = full_rect.y;
            for member in &list.members {
                let mut member_geom = full_rect;
                member_geom.stacked = Some(next_synthetic_stack_id);
                member_geom.y = running_y;
                if *member == list.visible {
                    member_geom.rows.set_inner(
                        full_rect
                            .rows
                            .as_usize()
                            .saturating_sub(collapsed_member_count),
                    );
                } else {
                    member_geom.rows = Dimension::fixed(1);
                    member_geom.logical_position =
                        self.suppressed_panes.get(member).and_then(|suppressed| {
                            suppressed.pane.position_and_size().logical_position
                        });
                }
                running_y += member_geom.rows.as_usize();
                synthetic_geoms.insert(*member, member_geom);
            }
            next_synthetic_stack_id += 1;
        }
        synthetic_geoms
    }
    pub fn hidden_stack_list_members_for_serialization(
        &self,
        floating: bool,
    ) -> Vec<(PaneId, &dyn Pane, PaneGeom)> {
        let synthetic_geoms = self.stack_list_serialization_geoms();
        let mut hidden_members = vec![];
        for list in self.stack_lists.values() {
            let floating_pane = self.floating_panes.get_pane(list.visible);
            if floating_pane.is_some() != floating {
                continue;
            }
            for member in &list.members {
                if *member == list.visible {
                    continue;
                }
                let Some(member_geom) = floating_pane
                    .map(|pane| pane.position_and_size())
                    .or_else(|| synthetic_geoms.get(member).copied())
                else {
                    continue;
                };
                // A member with a scrollback editor open on it *is* the editor -
                // `track_stack_list_member_id_swap` made the editor's id the member
                // id. What belongs in a layout is the pane being edited, which is
                // suppressed behind it.
                let resolved_pane = self
                    .scrollback_editor_subject(*member)
                    .or_else(|| self.suppressed_panes.get(member));
                if let Some(suppressed) = resolved_pane {
                    hidden_members.push((
                        suppressed.pane.pid(),
                        suppressed.pane.as_ref(),
                        member_geom,
                    ));
                }
            }
        }
        hidden_members
    }
    /// The pane, if any, whose place in the grid `stand_in` is *currently*
    /// holding, and which closing `stand_in` would therefore swap back in.
    ///
    /// Not the same question as `hidden_behind`: a stand-in that has itself been
    /// suppressed - a stack-list member with a scrollback editor open on it, which
    /// the user then clicked away from - holds nobody's place while it is off
    /// screen. Swapping into a slot it does not have would drop the pane.
    pub fn stands_in_for_a_pane(&self, stand_in: PaneId) -> Option<PaneId> {
        if !self.tiled_panes.panes_contain(&stand_in)
            && !self.floating_panes.panes_contain(&stand_in)
            && !self.overlay_panes.panes_contain(&stand_in)
        {
            return None;
        }
        self.hidden_behind(stand_in)
    }
    /// The pane, if any, that `stand_in` is holding the place of.
    fn hidden_behind(&self, stand_in: PaneId) -> Option<PaneId> {
        self.suppressed_panes
            .iter()
            .find(|(_, suppressed)| {
                suppressed
                    .restored_by
                    .map(|restored_by| restored_by.pane_id == stand_in)
                    .unwrap_or(false)
            })
            .map(|(pane_id, _)| *pane_id)
    }
    /// The pane whose scrollback `stand_in` is editing.
    pub(super) fn scrollback_editor_subject(&self, stand_in: PaneId) -> Option<&SuppressedPane> {
        self.suppressed_panes.values().find(|suppressed| {
            suppressed
                .restored_by
                .map(|restored_by| {
                    restored_by.is_scrollback_editor && restored_by.pane_id == stand_in
                })
                .unwrap_or(false)
        })
    }
    pub(super) fn refresh_or_dissolve_stack_list(&mut self, stack_list_id: StackListId) {
        let (member_count, visible) = match self.stack_lists.get(&stack_list_id) {
            Some(list) => (list.members.len(), list.visible),
            None => return,
        };
        if member_count <= 1 {
            self.dissolve_stack_list(stack_list_id);
        } else {
            self.reserved_top_rows
                .borrow_mut()
                .insert(visible, member_count + 1);
            self.tiled_panes.reapply_pane_frames();
            self.resize_stack_list_hidden_members(stack_list_id);
        }
    }
    pub(super) fn select_stack_list_member(
        &mut self,
        stack_list_id: StackListId,
        target_member: PaneId,
        client_id: Option<ClientId>,
    ) {
        let current_visible = match self.stack_lists.get(&stack_list_id) {
            Some(list) => list.visible,
            None => return,
        };
        if current_visible == target_member {
            return;
        }
        let target_box = match self.suppressed_panes.remove(&target_member) {
            Some(suppressed) => suppressed.pane,
            None => {
                log::error!(
                    "stack-list member {:?} not found in suppressed panes",
                    target_member
                );
                return;
            },
        };
        let removed_visible = if self.floating_panes.panes_contain(&current_visible) {
            match self
                .floating_panes
                .replace_pane(current_visible, target_box)
            {
                Ok(pane) => Some(pane),
                Err(error) => {
                    Err::<(), _>(error).non_fatal();
                    None
                },
            }
        } else {
            self.tiled_panes.replace_pane(current_visible, target_box)
        };
        if let Some(removed_visible) = removed_visible {
            // Whatever either of these two was standing in for stays suppressed
            // behind it, wherever it goes: an entry names its stand-in, not the
            // other way around, so a member changing places changes nothing.
            self.park_pane(removed_visible);
        }
        if let Some(list) = self.stack_lists.get_mut(&stack_list_id) {
            list.visible = target_member;
        }
        self.reserved_top_rows.borrow_mut().remove(&current_visible);
        if let Some(client_id) = client_id {
            if self.floating_panes.panes_contain(&target_member) {
                self.floating_panes.focus_pane(target_member, client_id);
            } else {
                self.tiled_panes.focus_pane(target_member, client_id);
            }
        }
        self.refresh_or_dissolve_stack_list(stack_list_id);
        self.set_force_render();
        self.set_should_clear_display_before_rendering();
    }
    pub(super) fn move_focus_within_stack_list(
        &mut self,
        client_id: ClientId,
        down: bool,
        wrap: bool,
    ) -> bool {
        let active_pane_id = match if self.floating_panes.panes_are_visible() {
            self.floating_panes.active_pane_id(client_id)
        } else {
            self.tiled_panes.get_active_pane_id(client_id)
        } {
            Some(id) => id,
            None => return false,
        };
        let stack_list_id = match self.stack_list_id_of_member(&active_pane_id) {
            Some(id) => id,
            None => return false,
        };
        let target_member = {
            let list = match self.stack_lists.get(&stack_list_id) {
                Some(l) => l,
                None => return false,
            };
            if list.visible != active_pane_id {
                return false;
            }
            let rank = match list.rank_of(&active_pane_id) {
                Some(r) => r,
                None => return false,
            };
            let target_rank = if down {
                if wrap && rank + 1 == list.members.len() {
                    0
                } else {
                    rank + 1
                }
            } else if rank == 0 {
                if !wrap {
                    return false;
                }
                list.members.len() - 1
            } else {
                rank - 1
            };
            match list.members.get(target_rank) {
                Some(member) => *member,
                None => return false,
            }
        };
        self.select_stack_list_member(stack_list_id, target_member, Some(client_id));
        true
    }
    pub(super) fn close_stack_list_member(&mut self, id: PaneId, exit_status: Option<i32>) -> bool {
        match self.remove_stack_list_member(id) {
            StackListRemoval::Removed(removed) => {
                if let (Some(exit_status), Some(mut removed)) = (exit_status, removed) {
                    removed.update_exit_status(exit_status);
                }
                true
            },
            StackListRemoval::DissolvedInPlace | StackListRemoval::NotRemoved => false,
        }
    }
    pub(super) fn remove_stack_list_member(&mut self, id: PaneId) -> StackListRemoval {
        let stack_list_id = match self.stack_list_id_of_member(&id) {
            Some(sid) => sid,
            None => return StackListRemoval::NotRemoved,
        };
        let is_visible = self
            .stack_lists
            .get(&stack_list_id)
            .map(|l| l.visible == id)
            .unwrap_or(false);
        let removed = if is_visible {
            let promote_target = match self.stack_list_promote_target(id) {
                Some(target) => target,
                None => {
                    self.dissolve_stack_list(stack_list_id);
                    self.reserved_top_rows.borrow_mut().remove(&id);
                    return StackListRemoval::DissolvedInPlace;
                },
            };
            let target_box = match self.suppressed_panes.remove(&promote_target) {
                Some(suppressed) => suppressed.pane,
                None => {
                    log::error!("stack-list promote target not found in suppressed panes");
                    return StackListRemoval::NotRemoved;
                },
            };
            let removed_visible = if self.floating_panes.panes_contain(&id) {
                match self.floating_panes.replace_pane(id, target_box) {
                    Ok(pane) => Some(pane),
                    Err(error) => {
                        Err::<(), _>(error).non_fatal();
                        None
                    },
                }
            } else {
                self.tiled_panes.replace_pane(id, target_box)
            };
            self.stack_list_of_member.remove(&id);
            self.reserved_top_rows.borrow_mut().remove(&id);
            if let Some(list) = self.stack_lists.get_mut(&stack_list_id) {
                list.members.retain(|m| *m != id);
                list.visible = promote_target;
            }
            self.refresh_or_dissolve_stack_list(stack_list_id);
            removed_visible
        } else {
            // A member with something suppressed behind it hands its place in the
            // list to that pane rather than taking it down with it - the member was
            // only ever standing in for it.
            let substitute_member = self.hidden_behind(id);
            let removed = self.suppressed_panes.remove(&id).map(|s| s.pane);
            if let Some(substitute_member) = substitute_member
                && let Some(suppressed) = self.suppressed_panes.get_mut(&substitute_member)
            {
                suppressed.restored_by = None;
            }
            self.stack_list_of_member.remove(&id);
            self.reserved_top_rows.borrow_mut().remove(&id);
            if let Some(substitute_member) = substitute_member {
                self.stack_list_of_member
                    .insert(substitute_member, stack_list_id);
                if let Some(list) = self.stack_lists.get_mut(&stack_list_id) {
                    for member in list.members.iter_mut() {
                        if *member == id {
                            *member = substitute_member;
                        }
                    }
                }
            } else if let Some(list) = self.stack_lists.get_mut(&stack_list_id) {
                list.members.retain(|m| *m != id);
            }
            self.refresh_or_dissolve_stack_list(stack_list_id);
            removed
        };
        self.set_force_render();
        self.set_should_clear_display_before_rendering();
        StackListRemoval::Removed(removed)
    }
    pub(super) fn track_stack_list_member_id_swap(&mut self, old_id: PaneId, new_id: PaneId) {
        let stack_list_id = match self.stack_list_id_of_member(&old_id) {
            Some(sid) => sid,
            None => return,
        };
        self.stack_list_of_member.remove(&old_id);
        self.stack_list_of_member.insert(new_id, stack_list_id);
        let mut swapped_visible_member = false;
        if let Some(list) = self.stack_lists.get_mut(&stack_list_id) {
            for member in list.members.iter_mut() {
                if *member == old_id {
                    *member = new_id;
                }
            }
            if list.visible == old_id {
                list.visible = new_id;
                swapped_visible_member = true;
            }
        }
        {
            let mut reserved = self.reserved_top_rows.borrow_mut();
            if let Some(reserved_rows) = reserved.remove(&old_id) {
                reserved.insert(new_id, reserved_rows);
            }
        }
        if swapped_visible_member {
            self.refresh_or_dissolve_stack_list(stack_list_id);
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
        }
    }
    fn dissolve_stack_list(&mut self, stack_list_id: StackListId) {
        if let Some(list) = self.stack_lists.get(&stack_list_id) {
            let members = list.members.clone();
            let floating = self.floating_panes.panes_contain(&list.visible);
            self.drop_stack_list_ledger(stack_list_id, &members);
            self.tiled_panes.reapply_pane_frames();
            if floating {
                self.floating_panes.set_pane_frames().non_fatal();
            }
        }
    }
    pub(super) fn drop_stack_list_ledger(
        &mut self,
        stack_list_id: StackListId,
        members: &[PaneId],
    ) {
        self.stack_lists.remove(&stack_list_id);
        let mut reserved = self.reserved_top_rows.borrow_mut();
        for member in members {
            self.stack_list_of_member.remove(member);
            reserved.remove(member);
        }
    }
    fn degroupify(&mut self, stack_list_id: StackListId) {
        let (members, visible) = match self.stack_lists.get(&stack_list_id) {
            Some(list) => (list.members.clone(), list.visible),
            None => return,
        };
        if self.floating_panes.panes_contain(&visible) {
            return;
        }
        let n = members.len();
        if n == 0 || !self.tiled_panes.panes_contain(&visible) {
            self.drop_stack_list_ledger(stack_list_id, &members);
            return;
        }
        let geoms = self.tiled_panes.stack_panes(visible, n);
        if geoms.len() != n {
            log::error!("degroupify: could not compute classic stack geoms");
            return;
        }
        for (i, member) in members.iter().enumerate() {
            let mut geom = geoms[i];
            if *member == visible {
                geom.logical_position = self
                    .tiled_panes
                    .get_pane(*member)
                    .and_then(|p| p.position_and_size().logical_position);
                self.tiled_panes.set_geom_for_pane_with_id(member, geom);
            } else {
                if let Some(mut suppressed) = self.suppressed_panes.remove(member) {
                    geom.logical_position = suppressed.pane.position_and_size().logical_position;
                    suppressed.pane.set_geom(geom);
                    self.tiled_panes
                        .add_pane_with_existing_geom(*member, suppressed.pane);
                }
            }
        }
        self.drop_stack_list_ledger(stack_list_id, &members);
        self.tiled_panes.expand_pane_in_stack(visible);
        self.tiled_panes.reapply_pane_frames();
        self.set_force_render();
        self.set_should_clear_display_before_rendering();
    }
    // the classic layout solver cannot see suppressed stack-list members; dissolve
    // lists into in-grid stacks first, groupify_all re-forms them on mode entry
    fn degroupify_all(&mut self) {
        let ids: Vec<StackListId> = self.stack_lists.keys().copied().collect();
        for id in ids {
            self.degroupify(id);
        }
    }
    pub(super) fn dissolve_stack_lists_for_classic_mutation(&mut self) {
        if self.stacked_pane_list_is_active() {
            self.degroupify_all();
        }
    }
    pub fn sync_stacked_pane_list_mode(&mut self) {
        if self.stacked_pane_list_is_active() {
            self.groupify_all();
        } else if self.has_stack_lists() {
            self.degroupify_all();
        }
    }
    // the inverse of degroupify_all: adopts classic in-grid stacks into stack
    // lists, suppressing all but the visible member of each
    pub(super) fn groupify_all(&mut self) {
        if self.tiled_panes.fullscreen_is_active() {
            return;
        }
        let mut stacks: HashMap<usize, Vec<(usize, PaneId, bool)>> = HashMap::new();
        for (id, pane) in self.tiled_panes.get_panes() {
            let geom = pane.position_and_size();
            if let Some(stack_id) = geom.stacked {
                let is_expanded = !geom.rows.is_fixed();
                stacks
                    .entry(stack_id)
                    .or_default()
                    .push((geom.y, *id, is_expanded));
            }
        }
        let mut converted_any = false;
        for (_stack_id, mut members) in stacks {
            if members.len() < 2 {
                continue;
            }
            members.sort_by_key(|(y, _, _)| *y);
            let representative = members[0].1;
            let rect = match self.tiled_panes.position_and_size_of_stack(&representative) {
                Some(r) => r,
                None => continue,
            };
            let visible = members
                .iter()
                .find(|(_, _, expanded)| *expanded)
                .map(|(_, id, _)| *id)
                .unwrap_or_else(|| members.last().map(|(_, id, _)| *id).unwrap());
            let ordered_ids: Vec<PaneId> = members.iter().map(|(_, id, _)| *id).collect();
            let logical_position = self
                .tiled_panes
                .get_pane(visible)
                .and_then(|p| p.position_and_size().logical_position);
            for (_, id, _) in &members {
                if *id == visible {
                    continue;
                }
                if let Some(pane) = self.tiled_panes.extract_pane(*id) {
                    self.park_pane(pane);
                }
            }
            if let Some(vpane) = self.tiled_panes.get_pane_mut(visible) {
                let mut g = rect;
                g.stacked = None;
                g.logical_position = logical_position;
                vpane.set_geom(g);
            }
            let stack_list_id = self.next_stack_list_id;
            self.next_stack_list_id += 1;
            for id in &ordered_ids {
                self.stack_list_of_member.insert(*id, stack_list_id);
            }
            self.stack_lists.insert(
                stack_list_id,
                StackList {
                    members: ordered_ids,
                    visible,
                },
            );
            self.refresh_or_dissolve_stack_list(stack_list_id);
            converted_any = true;
        }
        if converted_any {
            self.set_force_render();
            self.set_should_clear_display_before_rendering();
        }
    }
    pub(super) fn resize_all_stack_list_hidden_members(&mut self) {
        let ids: Vec<StackListId> = self.stack_lists.keys().copied().collect();
        for id in ids {
            self.resize_stack_list_hidden_members(id);
        }
    }
    pub(super) fn resize_stack_list_hidden_members(&mut self, stack_list_id: StackListId) {
        if let Some(list) = self.stack_lists.get(&stack_list_id) {
            let headers_visible = self.floating_stack_headers_visible(list);
            if self.floating_panes.fullscreen_pane_id() != Some(list.visible)
                && let Some(pane) = self.floating_panes.get_pane_mut(list.visible)
            {
                let mut offset = if pane.borderless() {
                    Offset::default()
                } else {
                    Offset::frame(1)
                };
                if headers_visible {
                    offset.top = list.members.len() + 1;
                }
                if pane.get_content_offset() != offset {
                    pane.set_content_offset(offset);
                    resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                        .non_fatal();
                }
            }
        }

        let (visible_geom, visible_offset, hidden_members) = {
            let list = match self.stack_lists.get(&stack_list_id) {
                Some(l) => l,
                None => return,
            };
            let (geom, offset) = match self
                .tiled_panes
                .get_pane(list.visible)
                .or_else(|| self.floating_panes.get_pane(list.visible))
            {
                Some(p) => (p.render_geom(), p.get_content_offset()),
                None => return,
            };
            let hidden: Vec<PaneId> = list
                .members
                .iter()
                .copied()
                .filter(|m| *m != list.visible)
                .collect();
            (geom, offset, hidden)
        };
        for member in hidden_members {
            if let Some(suppressed) = self.suppressed_panes.get_mut(&member) {
                let pane = &mut suppressed.pane;
                if pane.position_and_size() != visible_geom
                    || pane.get_content_offset() != visible_offset
                {
                    pane.set_geom(visible_geom);
                    pane.set_content_offset(visible_offset);
                    resize_pty!(pane, self.os_api, self.senders, self.character_cell_size)
                        .non_fatal();
                }
            }
        }
    }
    pub(super) fn stack_list_header_geom(
        mut rect: PaneGeom,
        content_offset: Offset,
        rank: usize,
    ) -> PaneGeom {
        rect.x += content_offset.left;
        rect.cols.set_inner(
            rect.cols
                .as_usize()
                .saturating_sub(content_offset.left + content_offset.right),
        );
        rect.y += rank;
        rect.rows = Dimension::fixed(1);
        rect.stacked = None;
        rect
    }

    pub(super) fn render_stack_list_headers(
        &mut self,
        output: &mut Output,
        client_id_override: Option<ClientId>,
        floating: bool,
    ) -> Result<()> {
        if self.stack_lists.is_empty() || (!floating && self.tiled_panes.fullscreen_is_active()) {
            return Ok(());
        }
        let mut connected_clients: HashSet<ClientId> =
            { self.connected_clients.borrow().iter().copied().collect() };
        if let Some(override_id) = client_id_override {
            connected_clients.insert(override_id);
        }
        if connected_clients.is_empty() {
            return Ok(());
        }
        let multiplayer = self.connected_clients_in_app.borrow().snapshot();
        let should_draw_pane_frames = self.pane_frame_style.draws_full_frames();
        let session_is_mirrored = self.stack_list_session_is_mirrored;
        let current_pane_group: HashMap<ClientId, Vec<PaneId>> =
            { self.current_pane_group.borrow().clone_inner() };
        let active_panes: HashMap<ClientId, PaneId> = connected_clients
            .iter()
            .filter_map(|c| {
                let pane = if floating {
                    self.floating_panes.active_pane_id(*c)
                } else {
                    self.tiled_panes.get_active_pane_id(*c)
                };
                pane.map(|p| (*c, p))
            })
            .collect();
        let client_modes: Vec<(ClientId, InputMode)> = {
            let mode_info = self.mode_info.borrow();
            connected_clients
                .iter()
                .map(|c| {
                    let mode = mode_info.get(c).unwrap_or(&self.default_mode_info).mode;
                    (*c, mode)
                })
                .collect()
        };
        let same_worktree_panes = self.same_worktree_panes(client_id_override);
        let lists: Vec<StackList> = self.stack_lists.values().cloned().collect();
        for list in lists {
            if self.floating_panes.panes_contain(&list.visible) != floating
                || (floating && !self.floating_stack_headers_visible(&list))
            {
                continue;
            }
            let z_index = if floating {
                self.floating_panes
                    .get_pane_z_index(list.visible)
                    .map(|index| index + 1)
            } else {
                None
            };

            let (rect, visible_content_offset) = match self
                .tiled_panes
                .get_pane(list.visible)
                .or_else(|| self.floating_panes.get_pane(list.visible))
            {
                Some(p) => (p.render_geom(), p.get_content_offset()),
                None => continue,
            };
            let widest_member_titles: HashMap<_, _> = connected_clients
                .iter()
                .map(|client| {
                    let widest = list
                        .members
                        .iter()
                        .filter_map(|member| {
                            if *member == list.visible {
                                self.get_pane_with_id(*member)
                            } else {
                                self.suppressed_panes.get(member).map(|s| &*s.pane)
                            }
                        })
                        .map(|pane| {
                            pane.stack_list_entry_label(
                                same_worktree_panes
                                    .get(client)
                                    .is_some_and(|panes| panes.contains(&pane.pid())),
                            )
                            .width()
                                + if pane.agent_title().marker().is_some() {
                                    2
                                } else {
                                    0
                                }
                        })
                        .max()
                        .unwrap_or(0);
                    (*client, widest)
                })
                .collect();
            let stack_is_focused = active_panes.values().any(|p_id| *p_id == list.visible);
            for (rank, member) in list.members.iter().enumerate() {
                let header_geom = Self::stack_list_header_geom(rect, visible_content_offset, rank);
                let pane_box = if *member == list.visible {
                    if floating {
                        self.floating_panes.get_pane_mut(*member)
                    } else {
                        self.tiled_panes.get_pane_mut(*member)
                    }
                } else {
                    self.suppressed_panes.get_mut(member).map(|s| &mut s.pane)
                };
                let pane_box = match pane_box {
                    Some(p) => p,
                    None => continue,
                };
                let mut pane_contents_and_ui = PaneContentsAndUi::new(
                    pane_box,
                    output,
                    self.style,
                    &active_panes,
                    multiplayer,
                    z_index,
                    false,
                    false,
                    should_draw_pane_frames,
                    &self.mouse_hover_pane_id,
                    current_pane_group.clone(),
                    false,
                    false,
                    self.mouse_scroll_resize,
                    self.dimmed_clients.clone(),
                );
                pane_contents_and_ui.set_frame_geom_override(Some(header_geom));
                for (client_id, client_mode) in &client_modes {
                    pane_contents_and_ui.set_same_worktree(
                        same_worktree_panes
                            .get(client_id)
                            .is_some_and(|panes| panes.contains(member)),
                    );
                    pane_contents_and_ui.set_stack_list_entry(
                        widest_member_titles.get(client_id).copied(),
                        *member == list.visible,
                        stack_is_focused,
                    );
                    pane_contents_and_ui.render_pane_frame(
                        *client_id,
                        *client_mode,
                        session_is_mirrored,
                        false,
                        true,
                        true,
                        true,
                    )?;
                }
            }
            // Tiled panes without full frames reserve the right offset for a shared divider.
            let shared_divider_width = if !floating && !should_draw_pane_frames {
                visible_content_offset.right
            } else {
                0
            };
            let mut padding_chunks = vec![CharacterChunk::new(
                vec![
                    EMPTY_TERMINAL_CHARACTER;
                    rect.cols.as_usize().saturating_sub(shared_divider_width)
                ],
                rect.x,
                rect.y + list.members.len(),
            )];
            // The title geometry omits the side offsets, and the frame starts below the list.
            for rank in 0..list.members.len() {
                for (x, width) in [
                    (rect.x, visible_content_offset.left),
                    (
                        rect.x
                            + rect
                                .cols
                                .as_usize()
                                .saturating_sub(visible_content_offset.right),
                        visible_content_offset
                            .right
                            .saturating_sub(shared_divider_width),
                    ),
                ] {
                    if width > 0 {
                        padding_chunks.push(CharacterChunk::new(
                            vec![EMPTY_TERMINAL_CHARACTER; width],
                            x,
                            rect.y + rank,
                        ));
                    }
                }
            }
            if let Some((foreground, background)) = self.style.chrome_colors() {
                for chunk in &mut padding_chunks {
                    chunk.add_pane_defaults(Some(foreground.into()), Some(background.into()));
                }
            }
            output.add_character_chunks_to_multiple_clients(
                padding_chunks,
                connected_clients.iter().copied(),
                z_index,
            )?;
        }
        Ok(())
    }
}
