package com.multiplex.mobile.controller

import android.content.res.Configuration
import android.graphics.Bitmap
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.foundation.layout.wrapContentSize
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material.icons.outlined.Monitor
import androidx.compose.material.icons.outlined.Mouse
import androidx.compose.material.icons.outlined.TouchApp
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
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
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
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
    // `.card` in design/remote-screens/android.html: the picture fills the top of the card with
    // the badge over it, and the one action sits under it.
    val shape = RoundedCornerShape(16.dp)
    Column(
        modifier
            .fillMaxWidth()
            .clip(shape)
            .background(MaterialTheme.colorScheme.surfaceContainerLow)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, shape),
    ) {
        val picture = preview?.picture ?: lastPicture
        Box(
            Modifier
                .fillMaxWidth()
                .aspectRatio(
                    picture
                        ?.let { it.width.toFloat() / it.height.toFloat() }
                        ?.takeIf { it.isFinite() && it > 0f }
                        ?: (16f / 10f),
                )
                .background(Color(0xFF0B0D10))
                .then(if (unavailable == null) Modifier.clickable(onClick = onOpenScreen) else Modifier),
            contentAlignment = Alignment.Center,
        ) {
            if (picture != null) {
                Image(
                    bitmap = picture.asImageBitmap(),
                    contentDescription = stringResource(com.multiplex.mobile.R.string.screen_preview_description),
                    contentScale = ContentScale.Fit,
                    modifier = Modifier.fillMaxSize(),
                )
            } else if (unavailable == null) {
                CircularProgressIndicator()
            }
            ScreenBadge(caption(preview, unavailable), live = unavailable == null && picture != null)
        }
        Box(Modifier.padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 12.dp)) {
            Button(
                onClick = onOpenScreen,
                enabled = unavailable == null,
                shape = RoundedCornerShape(20.dp),
                modifier = Modifier.fillMaxWidth().height(40.dp),
            ) {
                Icon(Icons.Outlined.Monitor, contentDescription = null, modifier = Modifier.size(18.dp))
                Spacer(Modifier.size(8.dp))
                Text(stringResource(com.multiplex.mobile.R.string.screen_open))
            }
        }
    }
}

/** `.thumb .badge`: what the picture is, over its top-left corner. */
@Composable
private fun ScreenBadge(text: String, live: Boolean) {
    Row(
        Modifier
            .fillMaxSize()
            .padding(12.dp)
            .wrapContentSize(Alignment.TopStart)
            .clip(RoundedCornerShape(8.dp))
            .background(Color(0xD9111316))
            .padding(horizontal = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (live) {
            Box(
                Modifier
                    .size(7.dp)
                    .clip(CircleShape)
                    .background(com.multiplex.mobile.ui.SlateExtras.done),
            )
        }
        Text(
            text,
            style = MaterialTheme.typography.labelMedium,
            color = Color(0xFFE6E8EB),
            modifier = Modifier.heightIn(min = 26.dp).wrapContentHeight(),
        )
    }
}


@Composable
private fun caption(
    preview: RemoteScreenModel?,
    unavailable: ControllerScreenUnavailable?,
): String = when (unavailable) {
    ControllerScreenUnavailable.NotGranted ->
        stringResource(com.multiplex.mobile.R.string.screen_not_granted)
    ControllerScreenUnavailable.SharingOff ->
        stringResource(com.multiplex.mobile.R.string.screen_sharing_off)
    ControllerScreenUnavailable.Stopped ->
        stringResource(com.multiplex.mobile.R.string.screen_session_stopped)
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
    terminals: List<ControllerSessionSummary> = emptyList(),
    attached: ControllerTerminalUiState? = null,
    onSelectTerminal: (String) -> Unit = {},
    onCloseTerminal: () -> Unit = {},
    onRetryTerminal: () -> Unit = {},
    onRequestControl: () -> Unit = {},
    onReleaseControl: () -> Unit = {},
    onBytes: (ByteArray) -> Unit = {},
    onPaste: (String) -> Unit = {},
    onConfirmPaste: () -> Unit = {},
    onCancelPaste: () -> Unit = {},
    onViewportChanged: (Int, Int, Boolean) -> Unit = { _, _, _ -> },
) {
    var viewWidth by remember { mutableStateOf(0f) }
    var viewHeight by remember { mutableStateOf(0f) }
    var showKeyboard by remember { mutableStateOf(false) }
    var typed by remember { mutableStateOf("") }
    var showConnection by remember { mutableStateOf(false) }
    var showDisplays by remember { mutableStateOf(false) }
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

    // The picture gets the whole screen and the chrome floats over it, as
    // design/remote-screens/android.html has it: `.ficon` at the top, `.ftoolbar` and `.ctl` at
    // the bottom. Nothing takes a slice out of a screen a desktop is being drawn on.
    val landscape = LocalConfiguration.current.orientation == Configuration.ORIENTATION_LANDSCAPE
    val controls: @Composable () -> Unit = {
        Column(Modifier.wrapContentWidth()) {
            ScreenControls(
                model = model,
                showKeyboard = showKeyboard,
                typed = typed,
                onTyped = { typed = it },
                onToggleKeyboard = { showKeyboard = !showKeyboard },
                showDisplays = showDisplays,
                onShowDisplays = { showDisplays = it },
                onShowConnection = { showConnection = true },
                onClose = onClose,
            )
        }
    }

    val showTerminals = !landscape && terminals.isNotEmpty()
    Column(modifier.fillMaxSize().background(Color(0xFF07080A))) {
    Box(if (showTerminals) Modifier.fillMaxWidth() else Modifier.fillMaxSize()) {
        Box(
            if (showTerminals) {
                Modifier
                    .fillMaxWidth()
                    .aspectRatio(
                        model.surfaceSize.let { (width, height) ->
                            (width / height).takeIf { it.isFinite() && it > 0f } ?: (16f / 10f)
                        },
                    )
            } else {
                Modifier.fillMaxSize()
            }
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
            // `.overlay-top` in multiplex-mobile-flow.html: the way back, and what this phone is
            // doing with the computer right now.
            Row(
                Modifier
                    .align(Alignment.TopCenter)
                    .fillMaxWidth()
                    .statusBarsPadding()
                    .padding(12.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                ScreenPill(
                    text = stringResource(com.multiplex.mobile.R.string.back),
                    leading = Icons.AutoMirrored.Outlined.ArrowBack,
                    onClick = onClose,
                )
                Text(
                    model.zoomLabel,
                    color = Color(0xFFE6E8EB),
                    style = MaterialTheme.typography.labelMedium,
                    modifier = Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .background(Color(0xCC111316))
                        .padding(horizontal = 12.dp, vertical = 6.dp),
                )
                Spacer(Modifier.weight(1f))
                ScreenPill(
                    text = stringResource(
                        if (model.isDriving) {
                            com.multiplex.mobile.R.string.screen_you_have_control
                        } else {
                            com.multiplex.mobile.R.string.screen_watching
                        },
                    ),
                    on = model.isDriving,
                )
            }
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
        if (weak) {
            Box(Modifier.align(Alignment.TopCenter).padding(top = 56.dp)) {
                WeakConnectionBanner(onDetails = { showConnection = true })
            }
        }
        // `.ftoolbar`: the bar floats over the picture when the picture has the screen to itself.
        // With terminals under it the picture is short, so the bar sits between the two instead
        // of covering the desktop it is there to show.
        if (!showTerminals) {
            Box(
                Modifier
                    .align(Alignment.BottomCenter)
                    .navigationBarsPadding()
                    .padding(horizontal = 12.dp, vertical = if (landscape) 8.dp else 24.dp)
                    .clip(RoundedCornerShape(28.dp))
                    .background(MaterialTheme.colorScheme.surfaceContainerLow.copy(alpha = 0.94f)),
            ) { controls() }
        }
    }
        if (showTerminals) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .background(MaterialTheme.colorScheme.surfaceContainerLow),
                contentAlignment = Alignment.Center,
            ) { controls() }
        }
        if (showTerminals) {
            ScreenTerminals(
                terminals = terminals,
                attached = attached,
                onSelect = onSelectTerminal,
                onClose = onCloseTerminal,
                onRetry = onRetryTerminal,
                onRequestControl = onRequestControl,
                onReleaseControl = onReleaseControl,
                onBytes = onBytes,
                onPaste = onPaste,
                onConfirmPaste = onConfirmPaste,
                onCancelPaste = onCancelPaste,
                onViewportChanged = onViewportChanged,
                modifier = Modifier.weight(1f),
            )
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


/** The bar under a computer's screen: what may be typed, who is driving, and the way out. */
@Composable
private fun ScreenControls(
    model: RemoteScreenModel,
    showKeyboard: Boolean,
    typed: String,
    onTyped: (String) -> Unit,
    onToggleKeyboard: () -> Unit,
    showDisplays: Boolean,
    onShowDisplays: (Boolean) -> Unit,
    onShowConnection: () -> Unit,
    onClose: () -> Unit,
) {
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
                    onTyped("")
                }
            },
            label = { Text(stringResource(com.multiplex.mobile.R.string.screen_type_here)) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp),
        )
    }
    // `.overlay-bottom` in multiplex-mobile-flow.html: taking control, giving it back, the
    // keyboard, and the way to this computer's terminals.
    Row(
        Modifier.padding(horizontal = 8.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        when {
            !model.canControlPointer && !model.canControlKeyboard -> ScreenPill(
                text = stringResource(
                    com.multiplex.mobile.R.string.screen_watch_only_not_granted,
                ),
                enabled = false,
            )
            model.control == com.multiplex.screens.ScreenControlHolder.YOU -> {
                ScreenPill(
                    text = stringResource(com.multiplex.mobile.R.string.screen_give_back_control),
                    onClick = { model.releaseControl() },
                )
                if (model.canControlKeyboard) {
                    ScreenPill(
                        text = stringResource(com.multiplex.mobile.R.string.screen_keyboard),
                        leading = Icons.Outlined.Keyboard,
                        on = showKeyboard,
                        onClick = onToggleKeyboard,
                    )
                }
            }
            else -> ScreenPill(
                text = stringResource(com.multiplex.mobile.R.string.screen_take_control),
                primary = true,
                enabled = model.control != com.multiplex.screens.ScreenControlHolder.ANOTHER_DEVICE,
                onClick = { model.requestControl() },
            )
        }
        if (model.canControlPointer) {
            ScreenPill(
                text = model.pointerMode.title,
                leading = if (model.pointerMode == RemotePointerMode.TOUCH) {
                    Icons.Outlined.TouchApp
                } else {
                    Icons.Outlined.Mouse
                },
                enabled = model.isDriving,
                onClick = {
                    model.pointerMode = if (model.pointerMode == RemotePointerMode.TOUCH) {
                        RemotePointerMode.TRACKPAD
                    } else {
                        RemotePointerMode.TOUCH
                    }
                },
            )
        }
        if (model.displays.size > 1) {
            Box {
                ScreenPill(
                    text = stringResource(com.multiplex.mobile.R.string.screen_displays),
                    leading = Icons.Outlined.Monitor,
                    onClick = { onShowDisplays(true) },
                )
                androidx.compose.material3.DropdownMenu(
                    expanded = showDisplays,
                    onDismissRequest = { onShowDisplays(false) },
                ) {
                    model.displays.forEach { display ->
                        androidx.compose.material3.DropdownMenuItem(
                            text = { Text(display.name) },
                            onClick = {
                                onShowDisplays(false)
                                model.watch(display)
                            },
                        )
                    }
                }
            }
        }
        ScreenPill(
            text = stringResource(com.multiplex.mobile.R.string.screen_connection),
            leading = Icons.Outlined.Info,
            onClick = onShowConnection,
        )
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

/**
 * The computer's terminals, under its screen.
 *
 * A desktop is wider than it is tall, so a portrait phone has room left under the picture. The
 * terminals live there: one horizontal tab each, titles on top, and the chosen one running below.
 * It is a second connection, not a second turn — the screen keeps going while a terminal is used.
 */
@Composable
private fun ScreenTerminals(
    terminals: List<ControllerSessionSummary>,
    attached: ControllerTerminalUiState?,
    onSelect: (String) -> Unit,
    onClose: () -> Unit,
    onRetry: () -> Unit,
    onRequestControl: () -> Unit,
    onReleaseControl: () -> Unit,
    onBytes: (ByteArray) -> Unit,
    onPaste: (String) -> Unit,
    onConfirmPaste: () -> Unit,
    onCancelPaste: () -> Unit,
    onViewportChanged: (Int, Int, Boolean) -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(modifier.fillMaxWidth().background(MaterialTheme.colorScheme.background)) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(MaterialTheme.colorScheme.surfaceContainerLow)
                .horizontalScroll(rememberScrollState()),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            terminals.forEach { session ->
                val on = attached?.sessionId == session.id
                Column(
                    Modifier
                        .clickable { if (!on) onSelect(session.id) }
                        .padding(horizontal = 16.dp, vertical = 10.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Text(
                        isolated(session.title),
                        style = MaterialTheme.typography.labelLarge,
                        color = if (on) {
                            MaterialTheme.colorScheme.primary
                        } else {
                            MaterialTheme.colorScheme.onSurfaceVariant
                        },
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Spacer(Modifier.size(6.dp))
                    Box(
                        Modifier
                            .height(2.dp)
                            .width(if (on) 28.dp else 0.dp)
                            .background(MaterialTheme.colorScheme.primary),
                    )
                }
            }
        }
        HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        if (attached != null) {
            ControllerTerminalScreen(
                terminal = attached,
                onRetry = onRetry,
                onRequestControl = onRequestControl,
                onReleaseControl = onReleaseControl,
                onBytes = onBytes,
                onPaste = onPaste,
                onConfirmPaste = onConfirmPaste,
                onCancelPaste = onCancelPaste,
                onViewportChanged = onViewportChanged,
            )
        } else {
            Box(
                Modifier.fillMaxSize(),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    stringResource(com.multiplex.mobile.R.string.screen_pick_a_terminal),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

/** `.pillbtn` in multiplex-mobile-flow.html: a floating pill over the picture. */
@Composable
private fun ScreenPill(
    text: String,
    leading: androidx.compose.ui.graphics.vector.ImageVector? = null,
    on: Boolean = false,
    primary: Boolean = false,
    enabled: Boolean = true,
    onClick: (() -> Unit)? = null,
) {
    val background = when {
        primary -> MaterialTheme.colorScheme.primary
        on -> MaterialTheme.colorScheme.secondaryContainer
        else -> MaterialTheme.colorScheme.surfaceContainerLow.copy(alpha = 0.95f)
    }
    val foreground = when {
        primary -> MaterialTheme.colorScheme.onPrimary
        on -> MaterialTheme.colorScheme.onSecondaryContainer
        else -> MaterialTheme.colorScheme.onSurface
    }
    Row(
        Modifier
            .clip(RoundedCornerShape(12.dp))
            .background(background)
            .then(
                if (onClick != null) Modifier.clickable(enabled = enabled, onClick = onClick)
                else Modifier,
            )
            .heightIn(min = 40.dp)
            .padding(horizontal = 14.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (leading != null) {
            Icon(leading, contentDescription = null, tint = foreground, modifier = Modifier.size(18.dp))
        }
        Text(
            text,
            style = MaterialTheme.typography.bodyMedium,
            color = if (enabled) foreground else com.multiplex.mobile.ui.SlateExtras.dimText,
        )
    }
}
