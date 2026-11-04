//! Selected output is reachable only through controlled context dispatch.
use super::*;
use crate::conversation::SelectedAdmission;
#[cfg(test)]
use crate::conversation::{ReaderContext, ReaderHandoff};
#[cfg(test)]
use std::rc::Rc;
use std::sync::Arc;
mod refusing;
pub(super) use refusing::RefusingBackend;

/// Process-local association only, never native qualification or stored evidence.
#[derive(Clone)]
#[cfg(test)]
pub(crate) struct ReaderBackendBinding(Rc<()>);
#[cfg(test)]
impl ReaderBackendBinding {
    fn new() -> Self {
        Self(Rc::new(()))
    }
    pub(crate) fn matches(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self::new()
    }
}

pub(crate) struct SelectedBackendFacade {
    #[cfg(test)]
    backend: Box<dyn DeviceBackend>,
    // Retain ownership until facade destruction without exposing unqualified effects.
    #[cfg(not(test))]
    _retained_backend: Box<dyn DeviceBackend>,
    admission: Arc<SelectedAdmission>,
    #[cfg(test)]
    binding: ReaderBackendBinding,
}
impl SelectedBackendFacade {
    pub(crate) fn new(admission: Arc<SelectedAdmission>, backend: Box<dyn DeviceBackend>) -> Self {
        Self {
            #[cfg(test)]
            backend,
            #[cfg(not(test))]
            _retained_backend: backend,
            admission,
            #[cfg(test)]
            binding: ReaderBackendBinding::new(),
        }
    }
    #[cfg(test)]
    pub(crate) fn binding(&self) -> ReaderBackendBinding {
        self.binding.clone()
    }
    pub(super) fn admission(&self) -> Arc<SelectedAdmission> {
        self.admission.clone()
    }
    /// Uses this facade's existing admission, without exposing its mutable backend.
    pub(super) fn inspect_buddy_binding(
        &self,
        accepted: &crate::storage::selection::SelectionToken,
        conversation: crate::storage::Uuid,
        document: crate::storage::Uuid,
        page: crate::storage::Uuid,
    ) -> Result<crate::conversation::SelectedBindingInspection> {
        self.admission
            .inspect_buddy_binding(accepted, conversation, document, page)
    }
    #[cfg(test)]
    pub(crate) fn dispatch(
        &mut self,
        context: &ReaderContext,
        ordinal: usize,
        handoff: &ReaderHandoff,
    ) -> Result<()> {
        context.dispatch_backend(&self.admission, &self.binding, ordinal, handoff, |permit| {
            match handoff {
                ReaderHandoff::SmartErase(input) => {
                    for (index, &(from, to)) in input.rectangles().iter().enumerate() {
                        let lower = ReaderHandoff::Erase {
                            bounds: [from.0, from.1, to.0 - from.0, to.1 - from.1],
                        };
                        permit.step(index, &lower, || self.backend.erase(from, to))?;
                    }
                    Ok(())
                }
                _ => permit.step(0, handoff, || {
                    match handoff {
                        ReaderHandoff::NextPage => {
                            self.backend
                                .navigate(xochitl_integration::NavigationDirection::Next)?;
                        }
                        ReaderHandoff::PreviousPage => {
                            self.backend
                                .navigate(xochitl_integration::NavigationDirection::Previous)?;
                        }
                        ReaderHandoff::BodyMode => self.backend.body_mode()?,
                        ReaderHandoff::Text(text) => self.backend.render_text(text)?,
                        ReaderHandoff::Symbol { x, y, text } => {
                            self.backend.bitmap(&positioned_symbol(*x, *y, text))?
                        }
                        ReaderHandoff::Erase {
                            bounds: [x, y, width, height],
                        } => self.backend.erase((*x, *y), (*x + *width, *y + *height))?,
                        ReaderHandoff::Progress(message) => {
                            self.backend.progress(message.as_deref())?
                        }
                        ReaderHandoff::SmartErase(_) => unreachable!(),
                    }
                    Ok(())
                }),
            }
        })
    }
}

pub(super) fn positioned_symbol(x: i32, y: i32, symbol: &str) -> Vec<Vec<bool>> {
    let bitmap = symbol_pool::SymbolPool::symbol_to_bitmap(symbol, 40);
    let offset_x = x.saturating_sub(20);
    let offset_y = y.saturating_sub(20);
    let mut positioned = vec![vec![false; 768]; 1024];
    for (dy, row) in bitmap.iter().enumerate() {
        for (dx, &pixel) in row.iter().enumerate() {
            let px = i64::from(offset_x) + dx as i64;
            let py = i64::from(offset_y) + dy as i64;
            if (0..768).contains(&px) && (0..1024).contains(&py) {
                positioned[py as usize][px as usize] = pixel;
            }
        }
    }
    positioned
}
