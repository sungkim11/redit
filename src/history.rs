pub(crate) struct UndoRedoHistory<T>
where
    T: Clone + PartialEq,
{
    undo: Vec<T>,
    redo: Vec<T>,
}

impl<T> Default for UndoRedoHistory<T>
where
    T: Clone + PartialEq,
{
    fn default() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

impl<T> UndoRedoHistory<T>
where
    T: Clone + PartialEq,
{
    pub(crate) fn begin_edit(&mut self, snapshot: T) {
        if self.undo.last() == Some(&snapshot) {
            return;
        }
        self.undo.push(snapshot);
        self.redo.clear();
    }

    pub(crate) fn undo(&mut self, current: T) -> Option<T> {
        let snapshot = self.undo.pop()?;
        self.redo.push(current);
        Some(snapshot)
    }

    pub(crate) fn redo(&mut self, current: T) -> Option<T> {
        let snapshot = self.redo.pop()?;
        self.undo.push(current);
        Some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::UndoRedoHistory;

    #[test]
    fn undo_and_redo_round_trip() {
        let mut history = UndoRedoHistory::default();
        let mut current = 1i32;

        history.begin_edit(current);
        current = 2;

        let undone = history
            .undo(current)
            .expect("undo should return a snapshot");
        assert_eq!(undone, 1);
        current = undone;

        let redone = history
            .redo(current)
            .expect("redo should return a snapshot");
        assert_eq!(redone, 2);
    }

    #[test]
    fn begin_edit_clears_redo_stack() {
        let mut history = UndoRedoHistory::default();

        history.begin_edit(1);
        let undo = history.undo(2).expect("undo should exist");
        assert_eq!(undo, 1);
        let _ = history.redo(1).expect("redo should exist");

        history.begin_edit(3);
        assert!(history.redo(4).is_none());
    }
}
