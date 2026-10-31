package com.metrix.writer

import org.json.JSONObject

/** Upgrade checks that use the current version model. Kept apart so older runners can build without it. */
internal object UpgradeChecks {
    fun versions(item: JSONObject): List<String> = ProcessingState.view(item, null, null).versions.map { it.kind.wire }
}
