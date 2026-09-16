package com.termirust.mobile.controller

import android.graphics.Bitmap
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

/**
 * The computer's screen on its own page: a picture about once a second, and the way in.
 */
@Composable
fun ControllerScreenPreviewCard(
    preview: RemoteScreenModel?,
    lastPicture: Bitmap?,
    unavailable: ControllerScreenUnavailable?,
    onOpenScreen: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("This Computer's Screen", style = MaterialTheme.typography.titleSmall)
        val picture = preview?.picture ?: lastPicture
        if (picture != null) {
            Image(
                bitmap = picture.asImageBitmap(),
                contentDescription = "Preview of this computer's screen",
                contentScale = ContentScale.Fit,
                modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)),
            )
        } else {
            Box(
                Modifier
                    .fillMaxWidth()
                    .aspectRatio(16f / 10f)
                    .clip(RoundedCornerShape(8.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant),
                contentAlignment = Alignment.Center,
            ) {
                if (unavailable == null) CircularProgressIndicator()
            }
        }
        Text(
            caption(preview, unavailable),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Button(
            onClick = onOpenScreen,
            enabled = unavailable == null,
            modifier = Modifier.fillMaxWidth(),
        ) { Text("Open Screen") }
    }
}

private fun caption(
    preview: RemoteScreenModel?,
    unavailable: ControllerScreenUnavailable?,
): String = when (unavailable) {
    ControllerScreenUnavailable.NotGranted ->
        "This computer has not given this phone screen access."
    is ControllerScreenUnavailable.Failed -> unavailable.reason
    null -> {
        val name = preview?.displayName
        when {
            preview == null -> "Waiting for the first picture."
            name == null -> "About one picture a second."
            else -> "$name · about one picture a second"
        }
    }
}

/**
 * One computer's screen: fitted, zoomed and panned by hand, and driven once the computer hands
 * this device control.
 */
@Composable
fun RemoteScreenView(
    model: RemoteScreenModel,
    reconnecting: Boolean,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var viewWidth by remember { mutableStateOf(0f) }
    var viewHeight by remember { mutableStateOf(0f) }
    var showKeyboard by remember { mutableStateOf(false) }
    var typed by remember { mutableStateOf("") }

    Column(modifier.fillMaxSize()) {
        Box(
            Modifier
                .fillMaxWidth()
                .weight(1f)
                .background(Color.Black)
                .onSizeChanged {
                    viewWidth = it.width.toFloat()
                    viewHeight = it.height.toFloat()
                }
                .pointerInput(model) {
                    detectTransformGestures { _, pan, zoom, _ ->
                        if (zoom != 1f) {
                            model.setZoom(model.zoom * zoom, viewWidth, viewHeight)
                        }
                        if (model.pointerMode == RemotePointerMode.TRACKPAD && model.isDriving) {
                            model.movePointer(pan.x, pan.y, viewWidth, viewHeight)
                        } else {
                            model.panBy(pan.x, pan.y, viewWidth, viewHeight)
                        }
                    }
                }
                .pointerInput(model) {
                    detectTapGestures(
                        onDoubleTap = { model.fit() },
                        onTap = { offset -> model.tap(offset.x, offset.y, viewWidth, viewHeight) },
                    )
                },
            contentAlignment = Alignment.Center,
        ) {
            val picture = model.picture
            when {
                picture != null -> Image(
                    bitmap = picture.asImageBitmap(),
                    contentDescription = "This computer's screen",
                    contentScale = ContentScale.Fit,
                    modifier = Modifier.fillMaxSize(),
                )
                model.state is RemoteScreenState.Closed ->
                    Text(
                        (model.state as RemoteScreenState.Closed).reason,
                        color = Color.White,
                        style = MaterialTheme.typography.bodySmall,
                    )
                else -> CircularProgressIndicator()
            }
            if (model.zoom > 1.01f && viewWidth > 0f) {
                RemoteScreenMinimap(
                    model.visibleRect(viewWidth, viewHeight),
                    model.surfaceSize,
                    Modifier.align(Alignment.TopEnd).padding(12.dp),
                )
            }
            Text(
                model.zoomLabel,
                color = Color.White,
                style = MaterialTheme.typography.labelSmall,
                modifier = Modifier.align(Alignment.TopStart).padding(12.dp),
            )
            if (reconnecting) {
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    CircularProgressIndicator(color = Color.White)
                    Text("Reconnecting…", color = Color.White)
                }
            }
        }
        if (showKeyboard && model.canControlKeyboard) {
            Row(
                Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(8.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                RemoteScreenKey.ACCESSORY.forEach { key ->
                    OutlinedButton(onClick = { model.sendKey(key) }) { Text(key.label) }
                }
            }
            TextField(
                value = typed,
                onValueChange = { text ->
                    if (text.isNotEmpty()) {
                        model.sendText(text)
                        typed = ""
                    }
                },
                label = { Text("Type on this computer") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp),
            )
        }
        Row(
            Modifier.fillMaxWidth().padding(8.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (model.canControlKeyboard) {
                TextButton(onClick = { showKeyboard = !showKeyboard }, enabled = model.isDriving) {
                    Text("Keyboard")
                }
            }
            if (model.canControlPointer) {
                TextButton(
                    onClick = {
                        model.pointerMode = if (model.pointerMode == RemotePointerMode.TOUCH) {
                            RemotePointerMode.TRACKPAD
                        } else {
                            RemotePointerMode.TOUCH
                        }
                    },
                    enabled = model.isDriving,
                ) { Text(model.pointerMode.title) }
            }
            Text(
                controlLabel(model),
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.weight(1f),
            )
            if (model.canControlPointer || model.canControlKeyboard) {
                TextButton(
                    onClick = {
                        if (model.control == com.termirust.screens.ScreenControlHolder.YOU) {
                            model.releaseControl()
                        } else {
                            model.requestControl()
                        }
                    },
                    enabled = model.control != com.termirust.screens.ScreenControlHolder.ANOTHER_DEVICE,
                ) {
                    Text(
                        if (model.control == com.termirust.screens.ScreenControlHolder.YOU) {
                            "Stop controlling"
                        } else {
                            "Take control"
                        },
                    )
                }
            }
            TextButton(onClick = onClose) { Text("Done") }
        }
    }
}

private fun controlLabel(model: RemoteScreenModel): String =
    when (model.control) {
        com.termirust.screens.ScreenControlHolder.YOU -> "You are controlling this computer"
        com.termirust.screens.ScreenControlHolder.ANOTHER_DEVICE ->
            "Another device is controlling it"
        com.termirust.screens.ScreenControlHolder.NOBODY -> "Watching only"
    }

/** Where the view is looking, when the picture is bigger than the view. */
@Composable
private fun RemoteScreenMinimap(
    visible: FloatArray,
    surface: Pair<Float, Float>,
    modifier: Modifier = Modifier,
) {
    val (surfaceWidth, surfaceHeight) = surface
    if (surfaceWidth <= 0f || surfaceHeight <= 0f) return
    val width = 76.dp
    Canvas(
        modifier
            .size(width, width * (surfaceHeight / surfaceWidth))
            .semantics { contentDescription = "Minimap" },
    ) {
        drawRect(Color.Black.copy(alpha = 0.35f))
        drawRect(
            color = Color.White.copy(alpha = 0.9f),
            topLeft = Offset(
                size.width * visible[0] / surfaceWidth,
                size.height * visible[1] / surfaceHeight,
            ),
            size = Size(
                size.width * visible[2] / surfaceWidth,
                size.height * visible[3] / surfaceHeight,
            ),
            style = Stroke(width = 2f),
        )
    }
}
