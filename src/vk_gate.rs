//! The action-7 VK toggle decision — pure, no side effects.
//!
//! This is the question `NOTES.md` exists to answer: given a Y/Triangle tap and
//! the current app state, should the virtual keyboard open (Xbox or spiral),
//! close, or stay blocked? Previously the decision was smeared across
//! `toggle_virtual_keyboard_combo` / `run_slot7_binding` / `dispatch_vk_toggle`
//! and interleaved with logging, latches, and thread spawns.
//!
//! Now both the CLI sim and the boot service build a [`GateInput`] and enact the
//! returned [`VkAction`]; the branching lives here and nowhere else.

use crate::Desktop;

/// Everything the gate needs to decide. Built by the caller from `App` state
/// (and the one `WARMUP_VK_SERVICE` env read) — the gate itself reads nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GateInput {
    pub mask_0x200_active: bool,
    pub slot7_action_type: u16,
    pub slot7_subtype: u16,
    /// A VK session already exists (`vk_session.is_some()`).
    pub vk_open: bool,
    pub modal_block_bit_4: bool,
    pub spiral_bit_9: bool,
    /// `WARMUP_VK_SERVICE` is set (boot/service path).
    pub service_mode: bool,
    pub input_desktop: Desktop,
}

/// Which desktop the Xbox VK window must attach to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateAttach {
    Input,
    Current,
}

/// Why a tap did not open/close the VK. Caller maps each to its log line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Blocked {
    /// Mask 0x200 absent -> slot 7 never resolves.
    MaskAbsent,
    /// Slot 7 exists but its action type does not queue a VK action.
    SlotNotQueueing,
    /// Slot 7 queues, but the queued action is not 7.
    QueuedNotSeven,
    /// App state bit 4 (modal) blocks the open.
    ModalBit4,
}

/// What the caller must enact after the gate decides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VkAction {
    Blocked(Blocked),
    Close,
    OpenXbox { attach: GateAttach },
    OpenSpiral,
}

/// The whole gate. Pure: same input -> same action, no I/O.
pub fn decide(i: GateInput) -> VkAction {
    if !i.mask_0x200_active {
        return VkAction::Blocked(Blocked::MaskAbsent);
    }

    // `run_slot7_binding`: action type 6 queues the subtype, anything else queues 0.
    let queued = if i.slot7_action_type == 6 {
        i.slot7_subtype
    } else {
        0
    };
    if queued != 7 {
        return VkAction::Blocked(if i.slot7_action_type == 6 {
            Blocked::QueuedNotSeven
        } else {
            Blocked::SlotNotQueueing
        });
    }

    // `dispatch_vk_toggle`: a second tap on an open VK closes it, before any block.
    if i.vk_open {
        return VkAction::Close;
    }
    if i.modal_block_bit_4 {
        return VkAction::Blocked(Blocked::ModalBit4);
    }
    if i.spiral_bit_9 {
        return VkAction::OpenSpiral;
    }
    VkAction::OpenXbox {
        attach: attach_for(i),
    }
}

/// Lock screen, logon, and UAC need `OpenInputDesktop`; the service path always
/// attaches to the input desktop, otherwise it follows the foreground desktop.
pub fn attach_for(i: GateInput) -> GateAttach {
    if i.service_mode {
        GateAttach::Input
    } else {
        match i.input_desktop {
            Desktop::Winlogon => GateAttach::Input,
            Desktop::Default => GateAttach::Current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default: mask active, slot 7 queues action 7, nothing blocking, no session.
    fn open_input() -> GateInput {
        GateInput {
            mask_0x200_active: true,
            slot7_action_type: 6,
            slot7_subtype: 7,
            vk_open: false,
            modal_block_bit_4: false,
            spiral_bit_9: false,
            service_mode: false,
            input_desktop: Desktop::Default,
        }
    }

    #[test]
    fn mask_absent_blocks() {
        let i = GateInput {
            mask_0x200_active: false,
            ..open_input()
        };
        assert_eq!(decide(i), VkAction::Blocked(Blocked::MaskAbsent));
    }

    #[test]
    fn slot_not_queueing_blocks() {
        let i = GateInput {
            slot7_action_type: 0,
            ..open_input()
        };
        assert_eq!(decide(i), VkAction::Blocked(Blocked::SlotNotQueueing));
    }

    #[test]
    fn queued_not_seven_blocks() {
        let i = GateInput {
            slot7_subtype: 3,
            ..open_input()
        };
        assert_eq!(decide(i), VkAction::Blocked(Blocked::QueuedNotSeven));
    }

    #[test]
    fn open_session_closes() {
        let i = GateInput {
            vk_open: true,
            ..open_input()
        };
        assert_eq!(decide(i), VkAction::Close);
    }

    #[test]
    fn close_wins_over_modal_block() {
        // A second tap closes even when the modal bit is set.
        let i = GateInput {
            vk_open: true,
            modal_block_bit_4: true,
            ..open_input()
        };
        assert_eq!(decide(i), VkAction::Close);
    }

    #[test]
    fn modal_bit_blocks_open() {
        let i = GateInput {
            modal_block_bit_4: true,
            ..open_input()
        };
        assert_eq!(decide(i), VkAction::Blocked(Blocked::ModalBit4));
    }

    #[test]
    fn spiral_flag_opens_spiral() {
        let i = GateInput {
            spiral_bit_9: true,
            ..open_input()
        };
        assert_eq!(decide(i), VkAction::OpenSpiral);
    }

    #[test]
    fn default_opens_xbox_current_desktop() {
        assert_eq!(
            decide(open_input()),
            VkAction::OpenXbox {
                attach: GateAttach::Current
            }
        );
    }

    #[test]
    fn winlogon_desktop_attaches_input() {
        let i = GateInput {
            input_desktop: Desktop::Winlogon,
            ..open_input()
        };
        assert_eq!(
            decide(i),
            VkAction::OpenXbox {
                attach: GateAttach::Input
            }
        );
    }

    #[test]
    fn service_mode_always_attaches_input() {
        // Even on the default desktop, the service path attaches to input.
        let i = GateInput {
            service_mode: true,
            input_desktop: Desktop::Default,
            ..open_input()
        };
        assert_eq!(
            decide(i),
            VkAction::OpenXbox {
                attach: GateAttach::Input
            }
        );
    }
}
