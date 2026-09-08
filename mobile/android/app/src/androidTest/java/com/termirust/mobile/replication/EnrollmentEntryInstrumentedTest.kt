package com.termirust.mobile.replication

import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import com.termirust.mobile.MainActivity
import org.junit.Rule
import org.junit.Test

class EnrollmentEntryInstrumentedTest {
    @get:Rule val compose = createAndroidComposeRule<MainActivity>()

    @Test fun enrollmentOpensFromDevicesAndReturnsWithoutPreparing() {
        compose.onNodeWithText("Devices").performClick()
        compose.onNodeWithContentDescription("Enrollment").performClick()
        compose.onNodeWithText("Enrollment").performClick()
        compose.onNodeWithContentDescription("Reload request").assertIsDisplayed()
        compose.onNodeWithContentDescription("Back to devices").performClick()
        compose.onNodeWithContentDescription("Enrollment").assertIsDisplayed()
    }
}
