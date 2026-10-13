package com.metrix.writer.ui.settings

import com.metrix.writer.CloudProviders
import com.metrix.writer.PRODUCT_NAME

/**
 * Every settings page: its route segment and the title the top bar and search
 * results show. One table, so titles, search and pages cannot drift apart.
 * Routes are `Settings/<id>`; old ids still resolve through [aliases].
 */
internal object SettingsRoutes {
    const val SETUP = "Setup"
    const val ENGINE = "Engine"
    const val SPEECH_MODEL = "Speech model"
    const val LANGUAGE = "Languages"
    const val POLISHING = "Polishing"
    const val POLISHING_MODEL = "Polishing model"
    const val POLISHING_OPTIONS = "Polishing options"
    const val POLISHING_INSTRUCTIONS = "Polishing instructions"
    const val MODELS = "Models"
    const val KEYS = "API keys"
    const val ACCOUNT = "Account"
    const val BACKUP = "Export & import"
    const val FLOATING = "Floating control"
    const val PERMISSIONS = "Permissions"
    const val APPEARANCE = "Appearance"
    const val STORAGE = "Storage & about"
    const val LEGACY = "Connection"
    const val LICENSES = "Licenses"

    /** Routes used by earlier releases and other activities (CloudSetupActivity opens "Cloud"). */
    private val aliases = mapOf("Cloud" to ENGINE, "Cleanup & delivery" to POLISHING)

    fun keyPage(provider: com.metrix.writer.CloudProvider) = "${provider.name} key"

    val titles: Map<String, String> get() = mapOf(
        SETUP to "Set up $PRODUCT_NAME", ENGINE to "Speech engine", SPEECH_MODEL to "Speech model",
        LANGUAGE to "Speech language", POLISHING to "Polishing", POLISHING_MODEL to "Polishing model",
        POLISHING_OPTIONS to "Polishing options", POLISHING_INSTRUCTIONS to "Polishing instructions",
        MODELS to "Phone models", KEYS to "API keys", ACCOUNT to "Account",
        BACKUP to "Export & import", FLOATING to "Floating control", PERMISSIONS to "Permissions", APPEARANCE to "Appearance",
        STORAGE to "Storage & about", LEGACY to "Legacy server connection", LICENSES to "Open-source licenses",
    ) + CloudProviders.all.associate { keyPage(it) to keyPage(it) }

    /** The page id a `Settings/…` route opens, with old ids mapped to their new pages. */
    fun page(route: String): String = route.substringAfter('/', "").let { aliases[it] ?: it }

    /** Top-bar title for a settings route, or null when [route] is not a settings page. */
    fun title(route: String): String? = if (route.startsWith("Settings/")) titles[page(route)] else null
}
