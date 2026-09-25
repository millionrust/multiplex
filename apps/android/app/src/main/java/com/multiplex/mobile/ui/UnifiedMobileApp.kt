package com.multiplex.mobile.ui

import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Devices
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.multiplex.mobile.MobileHostViewModel
import com.multiplex.mobile.controller.ControllerApp
import com.multiplex.mobile.controller.ControllerViewModel
import com.multiplex.mobile.controller.MobileRootDestination

@Composable
fun UnifiedMobileApp(
    connections: MobileHostViewModel,
    controller: ControllerViewModel,
    onImportVault: (String) -> Unit,
    onImportCredentialFile: () -> Unit,
) {
    // The phone opens on the computers it is paired with, which is what it is for: the SSH
    // connections it keeps itself are the other tab, not the front door.
    var destination by androidx.compose.runtime.saveable.rememberSaveable {
        mutableStateOf(MobileRootDestination.DEVICES)
    }
    val lifecycleOwner = LocalLifecycleOwner.current
    val controllerState by controller.state.collectAsState()
    // A terminal or a computer's screen fills the phone, so the shell's own bar steps out of the
    // way for both. The screen was left out, and the viewer opened with the tab bar still under
    // it and the picture squeezed above it.
    val controllerFullScreen = destination == MobileRootDestination.DEVICES &&
        (controllerState.activeTerminal != null || controller.screens.viewer != null)

    DisposableEffect(lifecycleOwner, destination) {
        var foregrounded = false
        fun foreground() {
            if (foregrounded) return
            foregrounded = true
            when (destination) {
                MobileRootDestination.CONNECTIONS -> connections.onForeground()
                MobileRootDestination.DEVICES -> controller.onForeground()
            }
        }
        fun background() {
            if (!foregrounded) return
            foregrounded = false
            when (destination) {
                MobileRootDestination.CONNECTIONS -> connections.onBackground()
                MobileRootDestination.DEVICES -> controller.onBackground()
            }
        }
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> foreground()
                Lifecycle.Event.ON_STOP -> background()
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        if (lifecycleOwner.lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) foreground()
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
            background()
        }
    }

    // The shell is themed here, so the navigation bar and rail are Slate too: they sat outside
    // both halves' own themes and stayed Material's stock purple.
    MultiplexMaterialTheme {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            if (maxWidth >= 840.dp && !controllerFullScreen) {
                Row(Modifier.fillMaxSize()) {
                    RouteRail(destination, onSelect = { destination = it })
                    RouteContent(
                        destination,
                        connections,
                        controller,
                        onImportVault,
                        onImportCredentialFile,
                        Modifier.weight(1f),
                    )
                }
            } else {
                Column(Modifier.fillMaxSize()) {
                    RouteContent(
                        destination,
                        connections,
                        controller,
                        onImportVault,
                        onImportCredentialFile,
                        Modifier.weight(1f),
                    )
                    if (!controllerFullScreen) {
                        RouteBar(destination, onSelect = { destination = it })
                    }
                }
            }
        }
    }
}

@Composable
private fun RouteContent(
    destination: MobileRootDestination,
    connections: MobileHostViewModel,
    controller: ControllerViewModel,
    onImportVault: (String) -> Unit,
    onImportCredentialFile: () -> Unit,
    modifier: Modifier,
) {
    when (destination) {
        MobileRootDestination.CONNECTIONS -> MultiplexApp(
            viewModel = connections,
            onImportVault = onImportVault,
            onImportCredentialFile = onImportCredentialFile,
            modifier = modifier,
        )
        MobileRootDestination.DEVICES -> ControllerApp(controller, modifier)
    }
}

@Composable
private fun RouteBar(selected: MobileRootDestination, onSelect: (MobileRootDestination) -> Unit) {
    NavigationBar(Modifier.navigationBarsPadding()) {
        NavigationBarItem(
            selected = selected == MobileRootDestination.CONNECTIONS,
            onClick = { onSelect(MobileRootDestination.CONNECTIONS) },
            icon = { Icon(Icons.Outlined.Terminal, contentDescription = null) },
            label = { Text(stringResource(com.multiplex.mobile.R.string.destination_connections)) },
        )
        NavigationBarItem(
            selected = selected == MobileRootDestination.DEVICES,
            onClick = { onSelect(MobileRootDestination.DEVICES) },
            icon = { Icon(Icons.Outlined.Devices, contentDescription = null) },
            label = { Text(stringResource(com.multiplex.mobile.R.string.destination_devices)) },
        )
    }
}

@Composable
private fun RouteRail(selected: MobileRootDestination, onSelect: (MobileRootDestination) -> Unit) {
    NavigationRail(Modifier.statusBarsPadding().navigationBarsPadding()) {
        NavigationRailItem(
            selected = selected == MobileRootDestination.CONNECTIONS,
            onClick = { onSelect(MobileRootDestination.CONNECTIONS) },
            icon = { Icon(Icons.Outlined.Terminal, contentDescription = null) },
            label = { Text(stringResource(com.multiplex.mobile.R.string.destination_connections)) },
        )
        NavigationRailItem(
            selected = selected == MobileRootDestination.DEVICES,
            onClick = { onSelect(MobileRootDestination.DEVICES) },
            icon = { Icon(Icons.Outlined.Devices, contentDescription = null) },
            label = { Text(stringResource(com.multiplex.mobile.R.string.destination_devices)) },
        )
    }
}
