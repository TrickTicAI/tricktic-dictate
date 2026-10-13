package com.metrix.writer

import android.content.Context
import com.metrix.writer.sync.PortableChanges
import org.json.JSONObject

/** One capture/retry selection boundary; phone mode never reads pairing or server capabilities. */
class DictationChoice(private val context: Context) {
    private val prefs = context.getSharedPreferences("phone-engine", Context.MODE_PRIVATE)
    fun isPhone() = prefs.getBoolean("enabled", false)
    fun model() = prefs.getString("model", "base")!!
    fun language(): String = context.getSharedPreferences("speech", Context.MODE_PRIVATE).let { speech ->
        speech.getString("language", null) ?: storedLanguage().also {
            check(speech.edit().putString("language", it).commit())
        }
    }
    /** The same language as [language], read without storing the default. */
    fun storedLanguage(): String = context.getSharedPreferences("speech", Context.MODE_PRIVATE).getString("language", null)
        ?: if (isPhone()) prefs.getString("language", "en")!! else DirectCloud(context).language()
    fun saveLanguage(language: String) {
        require(language in listOf("en", "he", "mixed"))
        check(context.getSharedPreferences("speech", Context.MODE_PRIVATE).edit().putString("language", language).commit())
        PortableChanges.changed(context)
    }
    fun save(phone: Boolean, model: String, language: String) {
        require(language in PhoneModels(context).languages(model))
        saveLanguage(language)
        check(prefs.edit().putBoolean("enabled", phone).putString("model", model).putString("language", language).commit())
        PortableChanges.changed(context)
    }
    /** Switches new dictations to the selected cloud provider; its key and model stay as configured. */
    fun useCloud(provider: String) {
        check(DirectCloud(context, provider).configured()) { "Add your ${CloudProviders.get(provider).name} key in Settings › API keys first." }
        DirectCloud(context, provider).select()
        check(prefs.edit().putBoolean("enabled", false).commit())
        PortableChanges.changed(context)
    }
    /** Switches new dictations to an installed phone model, keeping the language when the model supports it. */
    fun usePhone(model: String) {
        val models = PhoneModels(context)
        check(models.file(model).isFile) { "Download $model in Phone models first." }
        val language = language().takeIf { it in models.languages(model) } ?: models.languages(model).first()
        models.snapshot(model, language)
        save(true, model, language)
    }
    /** Content-free label of the engine new dictations use. */
    fun engineLabel(): String = if (isPhone()) "${model()} · on this phone"
        else DirectCloud.selectedProvider(context).let { provider -> DirectCloud(context, provider).let { cloud ->
            "${CloudProviders.get(provider).name} · ${CloudProviders.modelName(provider, cloud.model())}${if (cloud.configured()) "" else " · key needed"}" } }
    fun capture(): JSONObject {
        if (!isPhone()) return DirectCloud(context).captureChoice()
        val prefs = CompanionPreferences(context)
        val snapshot = PhoneModels(context).snapshot(model(), language())
        val options = prefs.options("phone", model(), language())
        snapshot.put("polish_policy", prefs.polishPolicy(options.getJSONObject("cleanup").optJSONObject("selection")))
        return JSONObject().put("server", "phone").put("engine", snapshot).put("selection", options.getJSONObject("selection"))
            .put("input_language", language()).put("options", options).put("delivery", prefs.delivery())
    }
    fun prepare(store: SessionStore, item: JSONObject) {
        if (item.has("pending")) return
        if (isPhone()) { val choice = capture(); store.preparePhone(item, choice.getJSONObject("options"), choice.getJSONObject("engine"), choice.getString("delivery")) }
        else DirectCloud(context).prepare(store, item)
    }
}
