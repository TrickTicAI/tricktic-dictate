package com.metrix.writer.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp

private val Paper = lightColorScheme(primary = Color(0xFFAA491B), onPrimary = Color.White,
    primaryContainer = Color(0xFFF5E1D3), onPrimaryContainer = Color(0xFF713110), background = Color(0xFFFAF7F2), onBackground = Color(0xFF302B27),
    surface = Color(0xFFFAF7F2), surfaceContainer = Color(0xFFF1EBE3), surfaceContainerLow = Color(0xFFFFFFFF),
    surfaceContainerHigh = Color(0xFFEDE5DB), onSurface = Color(0xFF302B27), onSurfaceVariant = Color(0xFF70665E),
    outlineVariant = Color(0xFFE6DED4), secondary = Color(0xFF696F56), secondaryContainer = Color(0xFFEAE3D8), onSecondaryContainer = Color(0xFF423A31))
private val Ink = darkColorScheme(primary = Color(0xFFF1AA80), onPrimary = Color(0xFF4D240F),
    primaryContainer = Color(0xFF503121), onPrimaryContainer = Color(0xFFFFDBC6), background = Color(0xFF1D1B19), onBackground = Color(0xFFF0E8E0),
    surface = Color(0xFF1D1B19), surfaceContainer = Color(0xFF292521), surfaceContainerLow = Color(0xFF24211E),
    surfaceContainerHigh = Color(0xFF35302A), onSurface = Color(0xFFF0E8E0), onSurfaceVariant = Color(0xFFBDB1A5),
    outlineVariant = Color(0xFF463D34), secondary = Color(0xFFB7C19A), secondaryContainer = Color(0xFF40382F), onSecondaryContainer = Color(0xFFF0E3D4))

private val Sage = Paper.copy(primary = Color(0xFF32654A), onPrimary = Color.White,
    primaryContainer = Color(0xFFD7EDDD), onPrimaryContainer = Color(0xFF173D28),
    background = Color(0xFFF4F8F2), surface = Color(0xFFF4F8F2), surfaceContainer = Color(0xFFE7EEE3),
    surfaceContainerHigh = Color(0xFFDDE8DA), onSurface = Color(0xFF24372B), onBackground = Color(0xFF24372B), onSurfaceVariant = Color(0xFF526450))
private val Ocean = Ink.copy(primary = Color(0xFF91CFED), onPrimary = Color(0xFF103747),
    primaryContainer = Color(0xFF234C60), onPrimaryContainer = Color(0xFFD0ECFA),
    background = Color(0xFF101F2A), surface = Color(0xFF101F2A), surfaceContainer = Color(0xFF192F3E),
    surfaceContainerLow = Color(0xFF152834), surfaceContainerHigh = Color(0xFF244153), onSurface = Color(0xFFE0EEF5), onBackground = Color(0xFFE0EEF5), onSurfaceVariant = Color(0xFFADC3D0))
fun writerColors(mode: String, systemDark: Boolean) = when(mode) {
    "sage" -> Sage; "ocean" -> Ocean; "dark" -> Ink; "light" -> Paper; else -> if (systemDark) Ink else Paper
}

// Resolve punctuation and mixed-script runs from their content, independently of RTL navigation.
private val WriterTypography = Typography().let { base ->
    fun text(style: androidx.compose.ui.text.TextStyle) = style.copy(textDirection = androidx.compose.ui.text.style.TextDirection.Content)
    base.copy(
        displayLarge = text(base.displayLarge), displayMedium = text(base.displayMedium), displaySmall = text(base.displaySmall),
        headlineLarge = text(base.headlineLarge).copy(fontWeight = FontWeight.SemiBold), headlineMedium = text(base.headlineMedium), headlineSmall = text(base.headlineSmall),
        titleLarge = text(base.titleLarge).copy(fontWeight = FontWeight.SemiBold), titleMedium = text(base.titleMedium), titleSmall = text(base.titleSmall),
        bodyLarge = text(base.bodyLarge), bodyMedium = text(base.bodyMedium), bodySmall = text(base.bodySmall),
        labelLarge = text(base.labelLarge), labelMedium = text(base.labelMedium), labelSmall = text(base.labelSmall)
    )
}

@Composable fun WriterTheme(mode: String = "system", content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = writerColors(mode, isSystemInDarkTheme()),
        shapes = Shapes(small = RoundedCornerShape(12.dp), medium = RoundedCornerShape(18.dp), large = RoundedCornerShape(24.dp)),
        typography = WriterTypography, content = content)
}

/** Small native vectors follow Writer's restrained line icon vocabulary. */
@Composable fun WriterIcon(name: String, modifier: Modifier = Modifier, tint: Color = MaterialTheme.colorScheme.onSurfaceVariant) {
    Canvas(modifier.size(24.dp)) {
        val scale = size.width / 24f
        fun p(x: Float, y: Float) = Offset(x * scale, y * scale)
        fun line(x: Float, y: Float, a: Float, b: Float) = drawLine(tint, p(x,y),p(a,b), 1.7f * scale, StrokeCap.Round)
        when(name) {
            "wave", "record" -> listOf(8f, 15f, 20f, 12f, 6f).forEachIndexed { i,h -> line(4f+i*4,12-h/2,4f+i*4,12+h/2) }
            "Home" -> { val path=Path().apply { moveTo(3*scale,10*scale); lineTo(12*scale,3*scale); lineTo(21*scale,10*scale); moveTo(5*scale,9*scale); lineTo(5*scale,21*scale); lineTo(19*scale,21*scale); lineTo(19*scale,9*scale) }; drawPath(path,tint,style=Stroke(1.7f*scale)); line(10f,21f,10f,14f);line(10f,14f,14f,14f);line(14f,14f,14f,21f) }
            "Statistics" -> { line(5f,20f,5f,13f);line(12f,20f,12f,5f);line(19f,20f,19f,9f) }
            "Settings" -> { for (y in listOf(6f,12f,18f)) {line(3f,y,21f,y);drawCircle(tint,2.4f*scale,p(if(y==12f) 15f else 8f,y))} }
            "Dictionary" -> { drawRoundRect(tint,p(4f,3f),androidx.compose.ui.geometry.Size(16*scale,18*scale),androidx.compose.ui.geometry.CornerRadius(2*scale),style=Stroke(1.7f*scale));line(8f,3f,8f,21f);line(11f,8f,17f,8f);line(11f,12f,16f,12f) }
            "History" -> { drawCircle(tint,8*scale,p(12f,12f),style=Stroke(1.7f*scale));line(12f,7f,12f,12f);line(12f,12f,16f,14f) }
            "back" -> {line(15f,5f,8f,12f);line(8f,12f,15f,19f)}
            "next" -> {line(9f,5f,16f,12f);line(16f,12f,9f,19f)}
            "check" -> {line(5f,12f,10f,17f);line(10f,17f,20f,6f)}
            "close" -> {line(6f,6f,18f,18f);line(18f,6f,6f,18f)}
            "search" -> {drawCircle(tint,6*scale,p(10.5f,10.5f),style=Stroke(1.7f*scale));line(15f,15f,20f,20f)}
            "add" -> {line(12f,4f,12f,20f);line(4f,12f,20f,12f)}
            "copy" -> {drawRoundRect(tint,p(8f,7f),androidx.compose.ui.geometry.Size(12*scale,14*scale),androidx.compose.ui.geometry.CornerRadius(2*scale),style=Stroke(1.7f*scale));line(4f,16f,4f,3f);line(4f,3f,15f,3f)}
            else -> {drawCircle(tint,8*scale,p(12f,12f),style=Stroke(1.7f*scale));line(12f,10f,12f,16f);drawCircle(tint,scale,p(12f,7f))}
        }
    }
}

fun formatDuration(ms: Long): String {
    val seconds = ms.coerceAtLeast(0) / 1000
    return if (seconds < 3600) "${seconds / 60}:${(seconds % 60).toString().padStart(2,'0')}" else "${seconds / 3600}h ${(seconds % 3600) / 60}m"
}
