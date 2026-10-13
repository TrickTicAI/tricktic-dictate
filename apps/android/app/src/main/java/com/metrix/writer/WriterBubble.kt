package com.metrix.writer

import android.content.Context
import android.content.Intent
import android.graphics.PixelFormat
import android.graphics.Rect
import android.view.Gravity
import android.view.WindowInsets
import android.view.WindowManager
import android.widget.TextView
import android.view.MotionEvent
import android.view.ViewConfiguration
import android.widget.FrameLayout
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.ui.unit.dp
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.platform.ViewCompositionStrategy
import androidx.compose.ui.semantics.*
import androidx.lifecycle.*
import androidx.savedstate.*
import com.metrix.writer.ui.*
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

/** A small non-focusable window. Capture and editor ownership never depend on its composition. */
class WriterBubble(private val context: Context, private val tap: () -> Unit) {
    private val wm = context.getSystemService(WindowManager::class.java)
    private val prefs = FloatingPreferences(context)
    private val density = context.resources.displayMetrics.density
    private fun dp(value: Int) = (value * density).roundToInt()
    private var view: FrameLayout? = null
    private var owner: OverlayOwner? = null
    private var overlayState by mutableStateOf("idle")
    private var controlSize by mutableIntStateOf(prefs.size)
    private var theme by mutableStateOf("system")
    private var level = 0f
    private var busy = false
    private var lastState = "idle"
    private var stateAt = 0L
    private var dragging = false
    private var availableWidth = 0
    private var safeLeft = 0
    private var safeTop = 0
    private var screenHeight = 0
    private var availableHeight = 0
    private var currentState = "idle"
    private var drop: TextView? = null
    private var dropParams: WindowManager.LayoutParams? = null
    private val params = WindowManager.LayoutParams(1,1,WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
        WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL or WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,
        PixelFormat.TRANSLUCENT).apply { gravity = Gravity.TOP or Gravity.LEFT; title = "$PRODUCT_NAME floating control" }
    @Volatile var diagnostic = "not_requested"
        private set
    private fun active() = currentState in listOf("starting", "recording", "saving", "processing", "verifying")
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
                if (event.actionMasked == MotionEvent.ACTION_MOVE &&
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
            // Exclude only this small window from Android's edge-back gesture.
            addOnLayoutChangeListener { v, _, _, _, _, _, _, _, _ ->
                v.systemGestureExclusionRects = listOf(Rect(0, 0, v.width, v.height))
            }
        }
        val composition = ComposeView(context).apply {
            setViewCompositionStrategy(ViewCompositionStrategy.DisposeOnViewTreeLifecycleDestroyed)
            setContent {
                WriterTheme(theme) {
                    Box {
                        when(overlayState) {
                            "recording", "starting", "saving", "processing", "verifying" -> RecordingPanel(overlayState,
                                { if (CaptureService.isActive()) SessionManager.get(context).capture.level else level },
                                { tap() }, { if(CaptureService.isActive()) CaptureService.stop(context,true) else SessionManager.get(context).cancelWork() }, bubble = true)
                            "recovery" -> Surface(onClick = { context.startActivity(Intent(context,HistoryActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) },
                                shape = MaterialTheme.shapes.medium, color = MaterialTheme.colorScheme.primaryContainer) {
                                Text(when(overlayState) { "failure" -> "Retry"; else -> "Saved" }, modifier = Modifier.padding(12.dp).semantics { contentDescription = "$PRODUCT_NAME result saved; open History" })
                            }
                            "failure", "uncertain" -> Row {
                                IdleControl(controlSize) { if (!busy) tap() }
                                IconButton(onClick = { context.startActivity(Intent(context, HistoryActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }, modifier = Modifier.semantics { contentDescription = if (overlayState == "uncertain") "Check insertion in History before copying" else "Open History to retry saved audio" }) { WriterIcon("info", tint = MaterialTheme.colorScheme.error) }
                            }
                            else -> IdleControl(controlSize) { if(!busy) tap() }
                        }
                    }
                }
            }
        }
        content.addView(composition, FrameLayout.LayoutParams(-1, -1))
        try { wm.addView(content,positioned(params)); view = content; owner = lifecycle }
        catch(error: RuntimeException) { lifecycle.destroy(); diagnostic = "attach_failed:${error.javaClass.simpleName}" }
    }
    private fun move(dx: Int, dy: Int) {
        val next = FloatingGeometry.clamp(params.x+dx,params.y+dy,params.width,params.height,availableWidth,if(dragging && !active()) screenHeight else availableHeight)
        params.x=next.first;params.y=next.second;update()
        val target = dropParams
        if(target != null) {
            val hovered = FloatingGeometry.dropped(params.x,params.y,params.width,params.height,target.x,target.y,target.width,target.height)
            drop?.alpha = if(hovered) 1f else .9f
            drop?.text = if(hovered) "Release to hide" else "Hide · ${prefs.minutes} min"
            drop?.scaleX = if(hovered) 1.05f else 1f
            drop?.scaleY = if(hovered) 1.05f else 1f
        }
    }
    private fun endDrag() {
        dragging=false
        val target = dropParams
        if(!active() && target != null && FloatingGeometry.dropped(params.x,params.y,params.width,params.height,target.x,target.y,target.width,target.height)) {
            prefs.snooze(); hide(); return
        }
        hideDrop()
        val placed = FloatingGeometry.clamp(params.x, params.y, params.width, params.height, availableWidth, availableHeight)
        params.x = placed.first; params.y = placed.second
        if(prefs.dock) params.x=FloatingGeometry.dock(params.x,params.width,availableWidth)
        prefs.position(params.x.toFloat()/(availableWidth-params.width).coerceAtLeast(1),params.y.toFloat()/(availableHeight-params.height).coerceAtLeast(1))
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
    fun show(visible: Boolean, busy: Boolean, keyboardTop: Int?, state: String = "idle", level: Float = 0f) {
        currentState=state;this.level=level;this.busy=busy
        if(!visible || !active() && (!prefs.enabled || prefs.snoozed())) { diagnostic="editor_hidden_or_snoozed";hide();return }
        if(state != lastState) { lastState=state;stateAt=android.os.SystemClock.elapsedRealtime() }
        overlayState=if(state == "success" || state == "recovery" && android.os.SystemClock.elapsedRealtime()-stateAt > 1800) "idle" else if(state=="error") "failure" else state
        controlSize=prefs.size;theme=AppearancePreferences(context).theme
        val expanded=overlayState in listOf("starting","recording","saving","processing")
        params.width=dp(if(expanded) { if(overlayState == "recording") 176 else 144 } else if(overlayState in listOf("failure", "uncertain")) controlSize.coerceAtLeast(48) + 48 else if(overlayState == "recovery") 88 else controlSize.coerceAtLeast(48))
        params.height=dp(if(expanded) 56 else controlSize.coerceAtLeast(48))
        val metrics=wm.currentWindowMetrics
        val insets=metrics.windowInsets
        val bars=insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
        val keyboard=insets.getInsets(WindowInsets.Type.ime())
        safeLeft=bars.left; safeTop=bars.top
        availableWidth=metrics.bounds.width()-bars.left-bars.right
        screenHeight=metrics.bounds.height()-bars.top-bars.bottom
        availableHeight=metrics.bounds.height()-bars.top-maxOf(bars.bottom,keyboard.bottom)
        if(keyboardTop != null) availableHeight=minOf(availableHeight,keyboardTop-bars.top)
        if(availableHeight < params.height || availableWidth < params.width) { hide();diagnostic="keyboard_leaves_no_space";return }
        if(!dragging) { params.x=((availableWidth-params.width)*prefs.x).roundToInt();params.y=((availableHeight-params.height)*prefs.y).roundToInt() }
        else {val p=FloatingGeometry.clamp(params.x,params.y,params.width,params.height,availableWidth,if(dragging && !active()) screenHeight else availableHeight);params.x=p.first;params.y=p.second}
        if(dragging && drop != null) {
            val target = dropParams!!
            target.x = (availableWidth-target.width)/2; target.y = (screenHeight-target.height-dp(16)).coerceAtLeast(0)
            runCatching { wm.updateViewLayout(drop, positioned(target)) }
        }
        // The visible control keeps the screen awake while it records.
        val awake=WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON
        params.flags=if(state in listOf("starting","recording")) params.flags or awake else params.flags and awake.inv()
        if(view == null) attach() else update()
        diagnostic="attached=${view!=null} x=${params.x} y=${params.y} size=${params.width} state=$overlayState dragging=$dragging safe_top=$safeTop area_height=$availableHeight screen_height=$screenHeight"
    }
    fun hide() { hideDrop();view?.let { try {wm.removeView(it)} catch (_:RuntimeException){} };view=null;owner?.destroy();owner=null;dragging=false }
}
