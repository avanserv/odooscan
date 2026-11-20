from .partner import ModernNote
from .upgrade import ModernCursorHolder


class ModernNoteHeir(ModernNote):
    def _x_author(self):
        return self._uid


class ModernCursorHeir(ModernCursorHolder):
    def cursor_again(self):
        return self._cr
