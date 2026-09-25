package com.multiplex.mobile.controller

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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.res.stringResource
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
import kotlinx.coroutines.delay

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
        Text(stringResource(com.multiplex.mobile.R.string.screen_this_computers_screen), style = MaterialTheme.typography.titleSmall)
        val picture = preview?.picture ?: lastPicture
        if (picture != null) {
            Image(
                bitmap = picture.asImageBitmap(),
                contentDescription = stringResource(com.multiplex.mobile.R.string.screen_preview_description),
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
        ) { Text(stringResource(com.multiplex.mobile.R.string.screen_open)) }
    }
}

@Composable
private fun caption(
    preview: RemoteScreenModel?,
    unavailable: ControllerScreenUnavailable?,
): String = when (unavailable) {
    ControllerScreenUnavailable.NotGranted ->
        stringResource(com.multiplex.mobile.R.string.screen_not_granted)
    is ControllerScreenUnavailable.Failed -> unavailable.reason
    null -> {
        val name = preview?.displayName
        when {
            preview == null -> stringResource(com.multiplex.mobile.R.string.screen_waiting_first_picture)
            name == null -> stringResource(com.multiplex.mobile.R.string.screen_about_one_picture)
            else -> "$name · " + stringResource(com.multiplex.mobile.R.string.screen_about_one_picture)
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
    var showConnection by remember { mutableStateOf(false) }
    // Nothing arrives to say the pictures stopped, so the view asks the clock once a second.
    var now by remember { mutableStateOf(System.currentTimeMillis()) }
    LaunchedEffect(model) {
        while (true) {
            delay(1_000)
            now = System.currentTimeMillis()
        }
    }
    val quietFor = model.lastPictureAtMillis?.let { now - it }
    val weak = model.state is RemoteScreenState.Watching && !reconnecting &&
        quietFor != null && quietFor > WEAK_AFTER_MILLIS

    Column(modifier.fillMaxSize()) {
        if (weak) {
            WeakConnectionBanner(onDetails = { showConnection = true })
        }
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
                    contentDescription = stringResource(com.multiplex.mobile.R.string.screen_description),
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
                    Text(stringResource(com.multiplex.mobile.R.string.screen_reconnecting), color = Color.White)
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
                label = { Text(stringResource(com.multiplex.mobile.R.string.screen_type_here)) },
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
                    Text(stringResource(com.multiplex.mobile.R.string.screen_keyboard))
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
                        if (model.control == com.multiplex.screens.ScreenControlHolder.YOU) {
                            model.releaseControl()
                        } else {
                            model.requestControl()
                        }
                    },
                    enabled = model.control != com.multiplex.screens.ScreenControlHolder.ANOTHER_DEVICE,
                ) {
                    Text(
                        if (model.control == com.multiplex.screens.ScreenControlHolder.YOU) {
                            stringResource(com.multiplex.mobile.R.string.screen_stop_controlling)
                        } else {
                            stringResource(com.multiplex.mobile.R.string.screen_take_control)
                        },
                    )
                }
            }
            else {
                // The absence of a button was the only sign that this computer never granted
                // pointer or keyboard.
                Text(
                    stringResource(com.multiplex.mobile.R.string.screen_control_not_granted),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            TextButton(onClick = { showConnection = true }) {
                Text(stringResource(com.multiplex.mobile.R.string.screen_connection))
            }
            TextButton(onClick = onClose) { Text(stringResource(com.multiplex.mobile.R.string.screen_done)) }
        }
    }

    if (showConnection) {
        RemoteScreenConnectionSheet(
            model = model,
            reconnecting = reconnecting,
            now = now,
            onDismiss = { showConnection = false },
        )
    }
}

/// How long a still picture goes before the view stops calling it a quiet screen.
private const val WEAK_AFTER_MILLIS = 4_000L

/**
 * Said when no picture has arrived for a while.
 *
 * A pause is usually the computer being quiet, so this never claims a fault: it says what is
 * observable, that nothing is lost, and offers the numbers behind it.
 */
@Composable
private fun WeakConnectionBanner(onDetails: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.secondaryContainer)
            .padding(horizontal = 14.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Column(Modifier.weight(1f)) {
            Text(
                stringResource(com.multiplex.mobile.R.string.screen_weak_title),
                style = MaterialTheme.typography.labelLarge,
            )
            Text(
                stringResource(com.multiplex.mobile.R.string.screen_weak_detail),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        TextButton(onClick = onDetails) {
            Text(stringResource(com.multiplex.mobile.R.string.screen_details))
        }
    }
}

/** What the session is doing, in numbers the person can read. */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
private fun RemoteScreenConnectionSheet(
    model: RemoteScreenModel,
    reconnecting: Boolean,
    now: Long,
    onDismiss: () -> Unit,
) {
    val (width, height) = model.surfaceSize
    androidx.compose.material3.ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                stringResource(com.multiplex.mobile.R.string.screen_connection),
                style = MaterialTheme.typography.titleMedium,
            )
            ConnectionFact(
                stringResource(com.multiplex.mobile.R.string.screen_route),
                stringResource(com.multiplex.mobile.R.string.screen_route_private),
            )
            ConnectionFact(
                stringResource(com.multiplex.mobile.R.string.screen_pictures),
                model.picturesDrawn.toString(),
            )
            ConnectionFact(
                stringResource(com.multiplex.mobile.R.string.screen_last_picture),
                lastPictureLabel(model.lastPictureAtMillis, now),
            )
            model.displayName?.let { name ->
                ConnectionFact(stringResource(com.multiplex.mobile.R.string.screen_display), name)
            }
            ConnectionFact(
                stringResource(com.multiplex.mobile.R.string.screen_size),
                if (width > 0f && height > 0f) {
                    stringResource(
                        com.multiplex.mobile.R.string.screen_size_value,
                        width.toInt(),
                        height.toInt(),
                    )
                } else {
                    stringResource(com.multiplex.mobile.R.string.screen_size_unknown)
                },
            )
            Text(
                stringResource(com.multiplex.mobile.R.string.screen_keeping_up),
                style = MaterialTheme.typography.titleSmall,
            )
            Text(
                if (reconnecting) {
                    stringResource(com.multiplex.mobile.R.string.screen_keeping_up_reconnecting)
                } else {
                    stringResource(com.multiplex.mobile.R.string.screen_keeping_up_quiet)
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun ConnectionFact(label: String, value: String) {
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(label, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.bodySmall)
    }
}

/** How long ago the last picture arrived, or that none has. */
@Composable
private fun lastPictureLabel(lastAtMillis: Long?, now: Long): String {
    if (lastAtMillis == null) {
        return stringResource(com.multiplex.mobile.R.string.screen_last_picture_none)
    }
    val seconds = ((now - lastAtMillis) / 1_000).toInt()
    return if (seconds <= 0) {
        stringResource(com.multiplex.mobile.R.string.screen_last_picture_now)
    } else {
        stringResource(com.multiplex.mobile.R.string.screen_last_picture_seconds, seconds)
    }
}

@Composable
private fun controlLabel(model: RemoteScreenModel): String =
    when (model.control) {
        com.multiplex.screens.ScreenControlHolder.YOU -> stringResource(com.multiplex.mobile.R.string.screen_you_control)
        com.multiplex.screens.ScreenControlHolder.ANOTHER_DEVICE ->
            stringResource(com.multiplex.mobile.R.string.screen_another_device_controls)
        com.multiplex.screens.ScreenControlHolder.NOBODY -> stringResource(com.multiplex.mobile.R.string.screen_watching_only)
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
    val label = stringResource(com.multiplex.mobile.R.string.screen_minimap)
    Canvas(
        modifier
            .size(width, width * (surfaceHeight / surfaceWidth))
            .semantics { contentDescription = label },
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
