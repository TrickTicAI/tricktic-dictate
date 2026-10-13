package com.metrix.writer.ui

import com.metrix.writer.PRODUCT_NAME
import java.text.Normalizer

/**
 * Settings search: the index the Settings page searches, and how it matches.
 *
 * Each page composes only when opened, so its text is written out here: one
 * entry per section or row, holding everything that section shows — its
 * description, options, buttons and notes — plus words people use that the
 * page does not. Providers, their models and phone models are added from the
 * live catalogs by [catalog]. Matching mirrors the desktop and web settings
 * search, so a misspelling such as "cleeboard" still finds the clipboard, and
 * every typed word has to match.
 *
 * [anchor] is the exact title of the group, Section or row on that page. When
 * the entry is chosen the page scrolls to it and marks it; without one the page
 * simply opens. When a page's text changes, change its entry here too.
 */
internal data class SettingEntry(val page: String, val title: String, val anchor: String?, val text: String)

internal object SettingsSearch {
    /** Labels shown under each result; the same names the top bar uses. */
    val pageNames: Map<String, String> get() = R.titles

    private val R = com.metrix.writer.ui.settings.SettingsRoutes

    val index = listOf(
        SettingEntry(R.SETUP, "Set up $PRODUCT_NAME", null, "Three steps to your first dictation. Finish setting up. 1. Allow recording and insertion. Microphone allowed. 2. Choose where speech is processed. 3. Try a dictation. Record on Home, then check your words in History. The floating control appears beside supported text fields after overlay and accessibility access are enabled. You can keep using your usual keyboard. onboarding first run getting started guide wizard"),
        SettingEntry(R.ENGINE, "Speech engine", null, "New dictations use the engine chosen here. Saved dictations keep the engine they were recorded with. There is no automatic fallback between cloud and phone. Active. dictation engine switch transcription speech recognition"),
        SettingEntry(R.ENGINE, "Cloud", "Cloud", "Audio goes directly to the provider, with your own key. Charges may apply. No key, tap to add one. cloud transcription online provider"),
        SettingEntry(R.ENGINE, "On this phone", "On this phone", "Runs offline, without uploading audio. Speed and accuracy vary by model and language. No phone model yet. Download one to dictate offline. local offline"),
        SettingEntry(R.SPEECH_MODEL, "Speech model", null, "Models for new dictations. Saved dictations keep the model they used. In use. Previously selected. Audio upload: up to 25 MB per file. Longer recordings stay saved for export or another engine. Check model access. whisper transcribe"),
        SettingEntry(R.KEYS, R.KEYS, null, "Your own keys for cloud speech and polishing. Each is encrypted on this phone, excluded from backups and never shown again. Providers. Checked. Not checked. Check failed. No key. Speech and polishing. Polishing only. In use. manage keys api key token secret credentials password"),
        SettingEntry(R.MODELS, "Phone models", null, "Models run on this phone without uploading audio. Phone inference is experimental; speed and accuracy vary by language and model. Model transfer. Pause transfer. The selected model was removed. Installed, in use. Partly downloaded. Not installed. Use this model. Verify file. SHA-256 verified. Remove model. Recordings and transcripts stay saved. Download. Resume download. Import file. offline local whisper license delete"),
        SettingEntry(R.LANGUAGE, "Speech language", "Speech language", "One choice for new dictations. Saved dictations keep their original language. English. Hebrew. Hebrew + English. $PRODUCT_NAME keeps the language you speak. Mixed speech keeps both Hebrew and English; it does not translate. Available choices depend on your selected model. accent dictate spoken input"),
        SettingEntry(R.POLISHING, "What gets inserted", "What gets inserted", "Choose the words $PRODUCT_NAME inserts. Your spoken words are always kept in History. As spoken. With my corrections: your dictionary's corrections and snippets are applied. Polished: grammar, sentences and paragraphs fixed online, as on the computer. If the chosen wording is unavailable, the saved original is inserted. wording words to insert dictionary raw delivery output"),
        SettingEntry(R.POLISHING, "Polishing", "Polishing", "Model. Instructions. Key. Try polishing: sends one sample sentence and shows the result. Only text is sent for polishing, never audio. Dictations your phone transcribes offline send their text to your polishing model. Polishing still runs in the background. Stop background polishing. cleanup clean up capitalization punctuation ai text model"),
        SettingEntry(R.POLISHING_INSTRUCTIONS, "Style", "Style", "Like the computer: full polish, grammar, sentences and paragraphs. Light: punctuation, capitals and spacing only. The style stays on this phone. super punctuation only"),
        SettingEntry(R.POLISHING_INSTRUCTIONS, "Instructions", "Instructions", "The instructions your polishing model follows, shared with your computer. Changes apply to the next dictation and sync to your computer. Reset to default. prompt wording edit"),
        SettingEntry(R.POLISHING_INSTRUCTIONS, "Writing style", "Writing style", "Rewrite strength: Light, Polished, Concise. Layout: Natural paragraphs, One line. Remove fillers and false starts. Resolve spoken corrections. Format lists automatically. Recognize spoken punctuation. The same preferences as the computer's Super mode. writing format"),
        SettingEntry(R.POLISHING_MODEL, "Polishing model", null, "Choose the model that polishes your text. Add a key, needed before its models can polish. Added by you. Your own models. text model llm"),
        SettingEntry(R.POLISHING_OPTIONS, "Thinking effort", "Thinking effort", "Sends nothing; the provider decides. Asks the model not to think. " + com.metrix.writer.CustomTextModels.REASONING.joinToString(". ") { com.metrix.writer.CustomTextModels.reasoningLabel(it) } + ". A model that does not accept the chosen level fails that polishing request; the saved transcript is kept. reasoning effort"),
        SettingEntry(R.POLISHING_OPTIONS, "Your own models", "Your own models", "Add a model by the ID its provider publishes. $PRODUCT_NAME cannot check it in advance. Model ID. Add model. Remove. None added. custom"),
        SettingEntry(R.FLOATING, "Preview", "A small space for your voice", "A small space for your voice. Idle. Recording. Processing. The recording preview is silent. During real capture, the waveform follows your microphone."),
        SettingEntry(R.FLOATING, "Control size", "Control size", "Show the floating control. Beside supported text fields. Floating control size in dp. The touch target stays at least 48 dp. enable bubble button overlay"),
        SettingEntry(R.FLOATING, "Placement", "Placement", "Free placement: keep where I drag. Your position is remembered, including the center. It adjusts to fit above the keyboard. Dock to nearest edge. After dragging, the control snaps to the nearest side. Reset position. bubble position"),
        SettingEntry(R.FLOATING, "Temporary hiding", "Temporary hiding", "Drag the idle control to Hide at the bottom of the screen. Release elsewhere to cancel. 10 minutes. 30 minutes. 60 minutes. Hidden now. Hide now. Show now. Hiding never discards audio. An active recording keeps its Finish and Cancel controls. snooze"),
        SettingEntry(R.APPEARANCE, "Theme", "Theme", "The floating control follows the same theme. Follow system. Paper, warm light. Ink, warm dark. Sage, calm green. Ocean, deep blue. dark light mode colour color palette"),
        SettingEntry(R.APPEARANCE, "Text size", "Text size", "Text size follows Android's font setting. $PRODUCT_NAME supports English, Hebrew and mixed text independently of the interface layout. font bigger larger rtl"),
        SettingEntry(R.PERMISSIONS, "Microphone", "Microphone", "Allowed: record your voice. Not allowed: tap to let $PRODUCT_NAME record your voice. mic audio permission"),
        SettingEntry(R.PERMISSIONS, "Display over other apps", "Display over other apps", "Allowed: floating control. Needed to show the microphone beside your keyboard. overlay draw permission"),
        SettingEntry(R.PERMISSIONS, "Text insertion", "Text insertion", "Connected: inserts into your editor. Enabled: waiting to connect. Reopen the service if this persists. Enable $PRODUCT_NAME voice insertion. Accessibility settings. On in Android Settings: paused until you accept. Declined: record in $PRODUCT_NAME and copy from History. Allow $PRODUCT_NAME to insert text using Accessibility? Accept. Decline. paste clipboard keyboard type permission consent disclosure"),
        SettingEntry(R.PERMISSIONS, "Recording notifications", "Recording notifications", "Allowed: Stop and Discard in the notification shade. Show Stop and Discard outside $PRODUCT_NAME. permission"),
        SettingEntry(R.STORAGE, "Your data", "Your data", "Saved on this phone. Dictations in private app storage. Export or delete single dictations from History; deleting also changes Statistics. Delete all audio and transcripts. Phone models work offline. Cloud dictation sends audio to your chosen provider; polishing sends transcript text. Uninstalling $PRODUCT_NAME or clearing app data removes local recordings and keys. erase wipe privacy storage delete history"),
        SettingEntry(R.STORAGE, "About $PRODUCT_NAME", PRODUCT_NAME, "Your voice. Your words. Version. license"),
        SettingEntry(R.STORAGE, "Legal", "Legal", "Privacy policy. Terms of use. Open-source licenses. Phone inference uses whisper.cpp (MIT). legal privacy terms license notices"),
        SettingEntry(R.BACKUP, "Export", "Export", "Save your settings and dictionary to a file you can import on another phone or computer. No account needed. Included: speech language, dictation engine, polishing and wording choices, polishing instructions, theme, and every dictionary word, correction and snippet. Never included: API keys, recordings, transcripts, microphone, floating control, permissions and downloaded models. Export to a file. backup save move transfer new phone"),
        SettingEntry(R.BACKUP, "Import", "Import", "Nothing changes until you review the preview and tap Apply. Settings. Dictionary. Merge: add to what's on this phone. Replace: remove dictionary entries that aren't in the file. Choose a file. Preview. Apply. Undo last import. restore load transfer new phone"),
        SettingEntry(R.LICENSES, "Open-source licenses", null, "License texts. Open-source licenses for every bundled component and phone model. whisper.cpp MIT Apache notices attribution"),
        SettingEntry(R.STORAGE, "Legacy server connection", "Legacy server connection", "Recover requests from earlier $PRODUCT_NAME versions. Advanced. Recover an older server request. New dictation uses your chosen cloud provider or a phone model. HTTPS endpoint. Device credential. Check connection. old server"),
    ) + Distribution.searchEntries + Account.searchEntries

    /** Entries for what the catalogs hold: each provider and its models, and each phone model. */
    fun catalog(phoneModels: List<String>): List<SettingEntry> =
        com.metrix.writer.CloudProviders.all.flatMap { provider ->
            listOfNotNull(
                if (provider.transcribes) SettingEntry(R.ENGINE, provider.name, provider.name, "cloud provider speech ${provider.note} " + provider.speech.joinToString(" ") { it.name })
                else SettingEntry(R.POLISHING_MODEL, provider.name, provider.name, "polishing text provider ${provider.note}"),
                SettingEntry(R.keyPage(provider), R.keyPage(provider), null, "${provider.name} api key token secret credential. Status. Paste API key. Paste. Get a key. Save and check. Import key from file. Check key. Remove key. Replace key. Add your key."),
                if (provider.text) SettingEntry(R.POLISHING_MODEL, "${provider.name} polishing", provider.name, "polishing text model " + provider.cleanup.joinToString(" ") { it.name }) else null,
            )
        } + phoneModels.map { SettingEntry("Models", it, it, "phone model offline download") }

    /**
     * The anchor the next opened page should scroll to. Read once, by the first
     * matching title that composes; not saved, so a recreated activity simply
     * shows the page from the top.
     */
    private var target: String? = null
    fun aim(anchor: String?) { target = anchor }
    fun consume(title: String): Boolean = (target == title).also { if (it) target = null }

    fun words(text: String): List<String> =
        Normalizer.normalize(text.lowercase(), Normalizer.Form.NFKD).replace(Regex("\\p{M}+"), "")
            .split(Regex("[^\\p{L}\\p{N}]+")).filter { it.isNotEmpty() }

    /** Optimal string alignment distance: edits, counting a swap of neighbours as one. */
    private fun distance(a: String, b: String): Int {
        val rows = Array(a.length + 1) { i -> IntArray(b.length + 1) { j -> if (i == 0) j else if (j == 0) i else 0 } }
        for (i in 1..a.length) for (j in 1..b.length) {
            val cost = if (a[i - 1] == b[j - 1]) 0 else 1
            rows[i][j] = minOf(rows[i - 1][j] + 1, rows[i][j - 1] + 1, rows[i - 1][j - 1] + cost)
            if (i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1]) rows[i][j] = minOf(rows[i][j], rows[i - 2][j - 2] + 1)
        }
        return rows[a.length][b.length]
    }

    /** 0 for no match up to 3 for the same word; misspellings allowed in proportion to length. */
    private fun wordMatch(query: String, word: String): Double {
        if (word == query) return 3.0
        if (word.startsWith(query)) return 2.5
        if (query.length >= 3 && query in word) return 2.0
        val allowed = when { query.length <= 3 -> 0; query.length <= 5 -> 1; else -> 2 }
        if (allowed == 0) return 0.0
        if (kotlin.math.abs(word.length - query.length) <= allowed && distance(query, word) <= allowed) return 1.5
        if (word.length > query.length && distance(query, word.take(query.length)) <= allowed) return 1.0
        return 0.0
    }

    /** Entries matching every word of [query], best first; the title counts most. */
    fun search(query: String, entries: List<SettingEntry> = index): List<SettingEntry> {
        val terms = words(query)
        if (terms.isEmpty()) return emptyList()
        return entries.mapIndexedNotNull { order, entry ->
            val fields = listOf(words(entry.title) to 3.0, words(pageNames[entry.page] ?: entry.page) to 2.0, words(entry.text) to 1.0)
            var score = 0.0
            for (term in terms) {
                val best = fields.maxOf { (list, weight) -> (list.maxOfOrNull { wordMatch(term, it) } ?: 0.0) * weight }
                if (best == 0.0) return@mapIndexedNotNull null
                score += best
            }
            Triple(entry, score, order)
        }.sortedWith(compareByDescending<Triple<SettingEntry, Double, Int>> { it.second }.thenBy { it.third }).map { it.first }
    }
}
