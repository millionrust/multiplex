use std::collections::VecDeque;

use termirust_domain::ConnectionBudget;

use crate::{ListenerError, ListenerErrorCode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueClass {
    Control,
    Terminal,
}

#[derive(Debug)]
pub struct BoundedFrameQueue {
    frames: VecDeque<(QueueClass, Vec<u8>)>,
    payload_bytes: usize,
    budget: ConnectionBudget,
}

impl BoundedFrameQueue {
    pub fn new(budget: ConnectionBudget) -> Result<Self, ListenerError> {
        budget.validate()?;
        Ok(Self {
            frames: VecDeque::new(),
            payload_bytes: 0,
            budget,
        })
    }

    pub fn push(&mut self, class: QueueClass, bytes: Vec<u8>) -> Result<(), ListenerError> {
        let frame_limit = match class {
            QueueClass::Control => self.budget.max_control_frame_bytes,
            QueueClass::Terminal => self.budget.max_terminal_frame_bytes,
        };
        if bytes.len() > frame_limit {
            return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
        }
        let new_payload_bytes = self
            .payload_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| ListenerError::new(ListenerErrorCode::QueueFull))?;
        if self.frames.len() >= self.budget.max_queue_frames
            || new_payload_bytes > self.budget.max_queue_payload_bytes
        {
            return Err(ListenerError::new(ListenerErrorCode::QueueFull));
        }
        self.frames.push_back((class, bytes));
        self.payload_bytes = new_payload_bytes;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<(QueueClass, Vec<u8>)> {
        let frame = self.frames.pop_front()?;
        self.payload_bytes = self.payload_bytes.saturating_sub(frame.1.len());
        Some(frame)
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }
}
