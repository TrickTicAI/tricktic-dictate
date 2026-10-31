package com.metrix.writer

import android.content.Context
import android.content.Intent
import android.graphics.PixelFormat
import android.graphics.Rect
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.HapticFeedbackConstants
import android.view.WindowInsets
import android.view.WindowManager
import android.widget.TextView
import android.view.MotionEvent
import android.view.ViewConfiguration
import android.widget.FrameLayout
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.*
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.platform.ViewCompositionStrategy
import androidx.compose.ui.unit.dp
import androidx.lifecycle.*
import androidx.savedstate.*
import com.metrix.writer.ui.*
import java.io.File
import java.util.concurrent.Executors
import kotlin.math.roundToInt

/** Standalone Compose ownership adapted from TrickTic's service-hosted overlay. */
internal class OverlayOwner : SavedStateRegistryOwner, ViewModelStoreOwner {
    private val registry = LifecycleRegistry(this)
    private val saved = SavedStateRegistryController.create(this)
    override val lifecycle: Lifecycle get() = registry
    override val savedStateRegistry: SavedStateRegistry get() = saved.savedStateRegistry
    override val viewModelStore = ViewModelStore()
    fun start() { saved.performAttach(); saved.performRestore(null); registry.currentState = Lifecycle.State.RESUMED }
    fun destroy() { registry.currentState = Lifecycle.State.DESTROYED; viewModelStore.clear() }
}

/**
 * A small non-focusable window. Capture and editor ownership never depend on its composition.
 * It follows the latest capture's session through the MW-ANDROID-01 processing view and shows
 * [FloatingState]s; the only text it types is an explicit "Insert here" / "Insert anyway".
 */
class WriterBubble(private val context: Context, private val tap: () -> Unit) {
    private val wm = context.getSystemService(WindowManager::class.java)
    private val prefs = FloatingPreferences(context)
    private val feedback = FeedbackPreferences(context)
    private val density = context.resources.displayMetrics.density
    private fun dp(value: Int) = (value * density).roundToInt()
    private val main = Handler(Looper.getMainLooper())
    private val worker = Executors.newSingleThreadExecutor { Thread(it, "writer-bubble").apply { isDaemon = true } }
    private var view: FrameLayout? = null
    private var owner: OverlayOwner? = null
    private var floating by mutableStateOf<FloatingState>(FloatingState.Idle)
    private var edge by mutableStateOf(false)
    private var cardMaxHeight by mutableStateOf(480.dp)
    private var cardMaxWidth by mutableStateOf(320.dp)
    private var message by mutableStateOf<String?>(null)
    private var timer by mutableStateOf(true)
    private var controlSize by mutableIntStateOf(prefs.size)
    private var theme by mutableStateOf("system")
    private var level = 0f
    private var busy = false
    private var dragging = false
    private var availableWidth = 0
    private var safeLeft = 0
    private var safeTop = 0
    private var screenHeight = 0
    private var availableHeight = 0
    private var contentWidth = 0
    private var contentHeight = 0
    private var drop: TextView? = null
    private var dropParams: WindowManager.LayoutParams? = null
    private val params = WindowManager.LayoutParams(1,1,WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
        WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL or WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,
        PixelFormat.TRANSLUCENT).apply { gravity = Gravity.TOP or Gravity.LEFT; title = "$PRODUCT_NAME floating control" }
    @Volatile var diagnostic = "not_requested"
        private set

    // The session the control follows: the latest capture, kept after capture ends.
    private data class Projection(val dictation: Dictation, val savedText: String?, val phone: Boolean)
    private var tracked: String? = null
    private var projection: Projection? = null
    private var projectionKey: Any? = null
    private var projecting = false
    private var lastStage: ProcessingStage? = null
    private var insertedSince: Long? = null
    private var collapsedFor: String? = null
    private var confirmingCancel = false
    private var blocked: String? = null
    private var edgeExpanded = false
    private val notified = HashMap<String, Boolean>() // session → unconfirmed

    /** Shows why recording did not start, in the control itself (for example microphone denied). */
    fun block(reason: String) { blocked = reason; main.post { refreshState(); if (view != null) update() } }

    private fun active() = floating !is FloatingState.Idle && floating !is FloatingState.Attention
    private fun card() = floating is FloatingState.NotInserted || floating is FloatingState.Unconfirmed || floating is FloatingState.Failed ||
        floating is FloatingState.Blocked || floating is FloatingState.CancelConfirm
    private fun fixedSize() = floating is FloatingState.Idle || floating is FloatingState.Attention

    private val actions = FloatingActions(
        start = { blocked = null; if (!busy) tap() },
        stop = { confirmingCancel = false; CaptureService.stop(context, false) },
        requestCancel = { confirmingCancel = true; refreshState() },
        continueRecording = { confirmingCancel = false; refreshState() },
        discard = { confirmingCancel = false; CaptureService.stop(context, true) },
        keep = { confirmingCancel = false; CaptureService.stop(context, false, keep = true) },
        // False only when the wait already ended; the next projection shows what happened.
        insertOriginal = { tracked?.let { id -> SessionManager.get(context).insertOriginalNow(id); projectionKey = null; refreshState() } },
        stopProcessing = { SessionManager.get(context).cancelWork() },
        insertHere = { anyway -> tracked?.let { id -> message = RecoveryActions.insertHere(context, id, anyway).message; refreshState() } },
        copy = { tracked?.let { id -> worker.execute {
            val copied = RecoveryActions.copy(context, id) != null
            main.post { message = if (copied) null else "Nothing saved to copy. Open it in History."; projectionKey = null; refreshState()
                if (copied) android.widget.Toast.makeText(context, "Copied", android.widget.Toast.LENGTH_SHORT).show() }
        } } },
        markHandled = { tracked?.let { id -> worker.execute { RecoveryActions.markHandled(context, id); main.post { projectionKey = null; refreshState() } } } },
        open = { tracked?.let { id -> collapsedFor = id; message = null; refreshState()
            context.startActivity(Intent(context, HistoryActivity::class.java).putExtra(RecoveryNotification.EXTRA_ROUTE, "History/$id")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)) } },
        retry = { tracked?.let { id -> collapsedFor = null; message = null; worker.execute {
            val manager = SessionManager.get(context)
            val problem = try {
                check(!manager.isBusy() && !CaptureService.isActive()) { "Finish current work, then retry." }
                DictationChoice(context).prepare(manager.store, manager.store.load(id)); manager.submit(id, 0); null
            } catch (error: Exception) { error.message ?: "Retry did not start. The audio is still saved." }
            main.post { message = problem; projectionKey = null; refreshState() }
        } } },
        collapse = { collapsedFor = tracked; message = null; refreshState() },
        expand = { if (edge) edgeExpanded = true else collapsedFor = null; refreshState(); update() },
        dismiss = { blocked = null; refreshState() },
        openSettings = { blocked = null; refreshState()
            context.startActivity(Intent(context, FloatingSetupActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) },
    )

    private fun attach() {
        if(view != null) return
        val lifecycle = OverlayOwner().also { it.start() }
        var downX = 0f; var downY = 0f; var originX = 0; var originY = 0
        val slop = ViewConfiguration.get(context).scaledTouchSlop
        val content = object : FrameLayout(context) {
            override fun onInterceptTouchEvent(event: MotionEvent): Boolean {
                if (event.actionMasked == MotionEvent.ACTION_DOWN) {
                    downX = event.rawX; downY = event.rawY; originX = params.x; originY = params.y
                }
                if (event.actionMasked == MotionEvent.ACTION_MOVE && !edge &&
                    (kotlin.math.abs(event.rawX - downX) > slop || kotlin.math.abs(event.rawY - downY) > slop)) {
                    dragging = true
                    if (!active()) showDrop()
                    return true // Cancels the child click; the editor never receives the gesture.
                }
                return false
            }
            override fun onTouchEvent(event: MotionEvent): Boolean {
                when (event.actionMasked) {
                    MotionEvent.ACTION_MOVE -> move((originX + event.rawX - downX).roundToInt() - params.x,
                        (originY + event.rawY - downY).roundToInt() - params.y)
                    MotionEvent.ACTION_UP -> endDrag()
                    MotionEvent.ACTION_CANCEL -> { dragging = false; hideDrop() }
                }
                return true
            }
        }.apply {
            setViewTreeLifecycleOwner(lifecycle);setViewTreeSavedStateRegistryOwner(lifecycle);setViewTreeViewModelStoreOwner(lifecycle)
            addOnLayoutChangeListener { v, _, _, _, _, _, _, _, _ ->
                // Exclude only this small window from Android's edge-back gesture.
                v.systemGestureExclusionRects = listOf(Rect(0, 0, v.width, v.height))
                // Wrapped states learn their size from layout; cards then re-anchor so they grow upward.
                if (!fixedSize() && (v.width != contentWidth || v.height != contentHeight)) {
                    contentWidth = v.width; contentHeight = v.height
                    main.post { if (view === v && !dragging) { place(); update() } }
                }
            }
        }
        val composition = ComposeView(context).apply {
            setViewCompositionStrategy(ViewCompositionStrategy.DisposeOnViewTreeLifecycleDestroyed)
            setContent {
                WriterTheme(theme) {
                    Box {
                        FloatingContent(floating, controlSize,
                            { if (CaptureService.isActive()) SessionManager.get(context).capture.level else level },
                            timer, edge, cardMaxHeight, cardMaxWidth, message, actions)
                    }
                }
            }
        }
        content.addView(composition, FrameLayout.LayoutParams(-2, -2))
        try { wm.addView(content,positioned(params)); view = content; owner = lifecycle }
        catch(error: RuntimeException) { lifecycle.destroy(); diagnostic = "attach_failed:${error.javaClass.simpleName}" }
    }
    private fun width() = if (fixedSize() || edge) params.width else contentWidth.takeIf { it > 0 } ?: dp(if (card()) 320 else 200).coerceAtMost(availableWidth)
    private fun height() = if (fixedSize() || edge) params.height else contentHeight.takeIf { it > 0 } ?: dp(if (card()) 200 else 56)
    private fun move(dx: Int, dy: Int) {
        val next = FloatingGeometry.clamp(params.x+dx,params.y+dy,width(),height(),availableWidth,if(dragging && !active()) screenHeight else availableHeight)
        params.x=next.first;params.y=next.second;update()
        val target = dropParams
        if(target != null) {
            val hovered = FloatingGeometry.dropped(params.x,params.y,width(),height(),target.x,target.y,target.width,target.height)
            drop?.alpha = if(hovered) 1f else .9f
            drop?.text = if(hovered) "Release to hide" else "Hide · ${prefs.minutes} min"
            drop?.scaleX = if(hovered) 1.05f else 1f
            drop?.scaleY = if(hovered) 1.05f else 1f
        }
    }
    private fun endDrag() {
        dragging=false
        val target = dropParams
        if(!active() && target != null && FloatingGeometry.dropped(params.x,params.y,width(),height(),target.x,target.y,target.width,target.height)) {
            prefs.snooze(); hide(); return
        }
        hideDrop()
        val placed = FloatingGeometry.clamp(params.x, params.y, width(), height(), availableWidth, availableHeight)
        params.x = placed.first; params.y = placed.second
        if(prefs.dock) params.x=FloatingGeometry.dock(params.x,width(),availableWidth)
        prefs.position(params.x.toFloat()/(availableWidth-width()).coerceAtLeast(1),params.y.toFloat()/(availableHeight-height()).coerceAtLeast(1))
        update()
    }
    private fun showDrop() {
        if(drop != null) return
        val targetWidth=minOf(dp(216), availableWidth)
        val targetHeight=dp(64)
        val layout=WindowManager.LayoutParams(targetWidth,targetHeight,WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL or WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,PixelFormat.TRANSLUCENT).apply {
            gravity=Gravity.TOP or Gravity.LEFT;x=(availableWidth-targetWidth)/2;y=(screenHeight-targetHeight-dp(16)).coerceAtLeast(0);softInputMode=WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING;title="$PRODUCT_NAME snooze target"
        }
        val target=TextView(context).apply {
            val colors = writerColors(theme, context.resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK == android.content.res.Configuration.UI_MODE_NIGHT_YES)
            text="Hide · ${prefs.minutes} min";textSize=16f;gravity=Gravity.CENTER;setTextColor(colors.onPrimaryContainer.toArgb())
            background=android.graphics.drawable.GradientDrawable().apply {setColor(colors.primaryContainer.toArgb());cornerRadius=dp(24).toFloat()}
            contentDescription="Drop to hide $PRODUCT_NAME for ${prefs.minutes} minutes"
            setOnClickListener { if (!active()) { prefs.snooze(); hide() } }
        }
        try { wm.addView(target,positioned(layout));drop=target;dropParams=layout } catch (_:RuntimeException) { }
    }
    private fun hideDrop() { drop?.let { try { wm.removeView(it) } catch (_:RuntimeException) {} };drop=null;dropParams=null }
    // Geometry is relative to the safe screen. Send explicit absolute coordinates to
    // WindowManager so consumed view insets cannot accumulate across drag/rotation.
    private fun positioned(source: WindowManager.LayoutParams) = WindowManager.LayoutParams().apply {
        copyFrom(source); x += safeLeft; y += safeTop; setFitInsetsTypes(0)
        softInputMode = WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING
    }
    private fun update() { view?.let { try {wm.updateViewLayout(it,positioned(params))} catch (_:RuntimeException) {hide()} } }

    /** Re-projects the tracked session off the main thread, only when something it depends on changed. */
    private fun requestProjection() {
        val id = tracked ?: return
        val manager = SessionManager.get(context)
        val key = listOf(id, ProcessingEvents.revision.value, File(context.filesDir, "deliveries").lastModified(), manager.store.revision, CaptureService.state, manager.state)
        if (key == projectionKey || projecting) return
        projectionKey = key; projecting = true
        worker.execute {
            val result = runCatching {
                val item = manager.store.load(id)
                val ledger = item.optString("original_target").takeIf { it.isNotEmpty() }?.let { token ->
                    runCatching { DeliveryStore(File(context.filesDir, "deliveries")).load(token) }.getOrNull()
                }
                val phone = item.optJSONObject("capture_choice")?.has("engine") ?: DictationChoice(context).isPhone()
                Projection(Dictation.from(item, ledger = ledger), ledger?.takeIf { it.has("text") }?.optString("text"), phone)
            }.getOrNull()
            main.post { projecting = false; if (tracked == id) { projection = result; observed(result) } }
        }
    }

    /** Stage transitions the control reacts to: the inserted pill, its haptic, and the recovery notification. */
    private fun observed(result: Projection?) {
        val d = result?.dictation ?: return
        val stage = d.view?.stage
        if (stage == ProcessingStage.INSERTED && lastStage != null && lastStage != ProcessingStage.INSERTED) {
            insertedSince = android.os.SystemClock.elapsedRealtime(); feedback(start = false)
        }
        lastStage = stage
        when (val attention = d.attention()) {
            Attention.NOT_INSERTED, Attention.UNCONFIRMED -> {
                val unconfirmed = attention == Attention.UNCONFIRMED
                // At most once per session and outcome; SessionManager's expired-lease path posts nothing itself.
                if (notified[d.id] != unconfirmed) {
                    notified[d.id] = unconfirmed
                    RecoveryNotification.post(context, d.id, unconfirmed, deliveryReason(d.ledger?.reasonCode.orEmpty()))
                }
            }
            else -> if (notified.remove(d.id) != null) RecoveryNotification.cancel(context, d.id)
        }
        refreshState()
        if (view != null) update()
    }

    private fun feedback(start: Boolean) {
        if (feedback.haptic) view?.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
        if (feedback.sound && context.getSystemService(android.media.AudioManager::class.java).ringerMode == android.media.AudioManager.RINGER_MODE_NORMAL) {
            runCatching {
                val tone = android.media.ToneGenerator(android.media.AudioManager.STREAM_NOTIFICATION, 60)
                tone.startTone(if (start) android.media.ToneGenerator.TONE_PROP_BEEP else android.media.ToneGenerator.TONE_PROP_ACK, 150)
                main.postDelayed({ tone.release() }, 400)
            }
        }
    }

    private var managerState = "idle"
    private fun refreshState() {
        val fixture = EditorFixture.enabled
        val now = android.os.SystemClock.elapsedRealtime()
        val next = FloatingView.from(FloatingInput(
            captureActive = !fixture && CaptureService.isActive(), capture = CaptureService.state,
            captureElapsedMs = (now - CaptureService.startedAtElapsed).coerceAtLeast(0),
            manager = if (fixture) "idle" else managerState, managerBusy = false,
            tracked = if (fixture) null else projection?.dictation, savedText = projection?.savedText,
            phone = projection?.phone ?: DictationChoice(context).isPhone(), polishLimitMs = CompanionPreferences(context).polishTimeoutMs(),
            now = now, insertedSince = insertedSince, collapsed = collapsedFor != null && collapsedFor == tracked,
            confirmingCancel = confirmingCancel, blocked = blocked))
        if (floating is FloatingState.Starting && next is FloatingState.Recording) feedback(start = true)
        if (next::class != floating::class) { message = null; contentWidth = 0; contentHeight = 0 }
        floating = next
    }

    fun show(visible: Boolean, busy: Boolean, keyboardTop: Int?, state: String = "idle", level: Float = 0f) {
        this.level=level;this.busy=busy;managerState=state
        CaptureService.activeSessionId?.let { id ->
            if (id != tracked) { tracked = id; projection = null; projectionKey = null; lastStage = null; insertedSince = null; collapsedFor = null; message = null }
        }
        if (!CaptureService.isActive()) confirmingCancel = false
        if (!EditorFixture.enabled) requestProjection()
        refreshState()
        timer = feedback.timer
        if(!visible || !active() && (!prefs.enabled || prefs.snoozed())) { diagnostic="editor_hidden_or_snoozed";hide();return }
        controlSize=prefs.size;theme=AppearancePreferences(context).theme
        val metrics=wm.currentWindowMetrics
        val insets=metrics.windowInsets
        val bars=insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
        val keyboard=insets.getInsets(WindowInsets.Type.ime())
        safeLeft=bars.left; safeTop=bars.top
        availableWidth=metrics.bounds.width()-bars.left-bars.right
        screenHeight=metrics.bounds.height()-bars.top-bars.bottom
        availableHeight=metrics.bounds.height()-bars.top-maxOf(bars.bottom,keyboard.bottom)
        if(keyboardTop != null) availableHeight=minOf(availableHeight,keyboardTop-bars.top)
        cardMaxWidth=(minOf(dp(320), availableWidth - dp(16)).coerceAtLeast(dp(200)) / density).dp
        cardMaxHeight=((availableHeight - dp(8)).coerceAtLeast(dp(160)) / density).dp
        val idle = controlSize.coerceAtLeast(48)
        if (fixedSize()) {
            params.width=dp(if (floating is FloatingState.Attention) idle + 48 else idle); params.height=dp(idle)
            edge = false; edgeExpanded = false
            if(availableHeight < params.height || availableWidth < params.width) { hide();diagnostic="keyboard_leaves_no_space";return }
        } else {
            // While recording, processing or holding a result, a keyboard that leaves no room turns the control into an edge tab (D-9).
            val needed = if (card()) minOf(height(), dp(200)) else dp(64)
            val noRoom = availableHeight < needed || availableWidth < dp(120)
            if (!noRoom) edgeExpanded = false
            edge = noRoom && !edgeExpanded
            if (edge) { params.width = dp(48); params.height = dp(64) }
            else { params.width = WindowManager.LayoutParams.WRAP_CONTENT; params.height = WindowManager.LayoutParams.WRAP_CONTENT }
        }
        place(noRoomExpanded = edgeExpanded)
        // The visible control keeps the screen awake while it records.
        val awake=WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON
        params.flags=if(floating is FloatingState.Starting || floating is FloatingState.Recording || floating is FloatingState.CancelConfirm) params.flags or awake else params.flags and awake.inv()
        if(view == null) attach() else update()
        diagnostic="attached=${view!=null} x=${params.x} y=${params.y} size=${width()} state=${if (edge) "edge_" else ""}${floating.name} dragging=$dragging safe_top=$safeTop area_height=$availableHeight screen_height=$screenHeight"
    }

    /** Positions the control: idle and pills at the saved relative place, cards growing upward from it. */
    private fun place(noRoomExpanded: Boolean = edgeExpanded) {
        if (dragging) {
            val p=FloatingGeometry.clamp(params.x,params.y,width(),height(),availableWidth,if(!active()) screenHeight else availableHeight);params.x=p.first;params.y=p.second
            dropParams?.let { target ->
                target.x = (availableWidth-target.width)/2; target.y = (screenHeight-target.height-dp(16)).coerceAtLeast(0)
                runCatching { wm.updateViewLayout(drop, positioned(target)) }
            }
            return
        }
        val w = width(); val h = height()
        if (edge) {
            params.x = if (prefs.x >= .5f) (availableWidth - w).coerceAtLeast(0) else 0
            params.y = (availableHeight - h).coerceAtLeast(0); return
        }
        params.x = ((availableWidth - w) * prefs.x).roundToInt().coerceIn(0, (availableWidth - w).coerceAtLeast(0))
        params.y = when {
            noRoomExpanded -> 0
            card() -> {
                val idle = dp(controlSize.coerceAtLeast(48))
                val bottom = ((availableHeight - idle) * prefs.y).roundToInt() + idle
                (bottom - h).coerceIn(0, (availableHeight - h).coerceAtLeast(0))
            }
            else -> ((availableHeight - h) * prefs.y).roundToInt().coerceIn(0, (availableHeight - h).coerceAtLeast(0))
        }
    }
    /** Ends the bubble with its service: removes the window and stops the projection worker. */
    fun release() { hide(); worker.shutdownNow() }
    fun hide() { hideDrop();view?.let { try {wm.removeView(it)} catch (_:RuntimeException){} };view=null;owner?.destroy();owner=null;dragging=false;contentWidth=0;contentHeight=0 }
}
