package com.openwhisper.android.accessibility

internal object FocusedEditorSearch {
    fun <T> find(
        roots: Iterable<T>,
        children: (T) -> Iterable<T>,
        matches: (T) -> Boolean,
        maxVisited: Int,
    ): T? {
        val pending = ArrayDeque<T>()
        roots.reversed().forEach(pending::add)
        var visited = 0

        while (pending.isNotEmpty() && visited < maxVisited) {
            val node = pending.removeLast()
            visited += 1
            if (matches(node)) return node
            children(node).reversed().forEach(pending::add)
        }
        return null
    }
}

internal object EditorNodeEligibility {
    fun isUsable(
        isEnabled: Boolean,
        isInputFocused: Boolean,
        isEditable: Boolean,
        supportsSetText: Boolean,
    ): Boolean = isEnabled &&
        isInputFocused &&
        (isEditable || supportsSetText)
}
