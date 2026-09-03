package com.termirust.mobile.ui

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
import com.termirust.mobile.MobileHostViewModel
import com.termirust.mobile.controller.ControllerApp
import com.termirust.mobile.controller.ControllerViewModel
import com.termirust.mobile.controller.MobileRootDestination

@Composable
fun UnifiedMobileApp(
    connections: MobileHostViewModel,
    controller: ControllerViewModel,
    onImportVault: (String) -> Unit,
    onImportCredentialFile: () -> Unit,
) {
    var destination by remember { mutableStateOf(MobileRootDestination.CONNECTIONS) }
    val lifecycleOwner = LocalLifecycleOwner.current
    val controllerState by controller.state.collectAsState()
    val controllerTerminalOpen = destination == MobileRootDestination.DEVICES &&
        controllerState.activeTerminal != null

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

    MaterialTheme {
        BoxWithConstraints(Modifier.fillMaxSize()) {
            if (maxWidth >= 840.dp && !controllerTerminalOpen) {
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
                    if (!controllerTerminalOpen) {
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
        MobileRootDestination.CONNECTIONS -> TermirustApp(
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
            label = { Text("Connections") },
        )
        NavigationBarItem(
            selected = selected == MobileRootDestination.DEVICES,
            onClick = { onSelect(MobileRootDestination.DEVICES) },
            icon = { Icon(Icons.Outlined.Devices, contentDescription = null) },
            label = { Text("Devices") },
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
            label = { Text("Connections") },
        )
        NavigationRailItem(
            selected = selected == MobileRootDestination.DEVICES,
            onClick = { onSelect(MobileRootDestination.DEVICES) },
            icon = { Icon(Icons.Outlined.Devices, contentDescription = null) },
            label = { Text("Devices") },
        )
    }
}
