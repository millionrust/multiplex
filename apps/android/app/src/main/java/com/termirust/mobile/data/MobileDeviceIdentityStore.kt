package com.termirust.mobile.data

import android.content.Context
import java.util.UUID

interface MobileDeviceIdentityStore {
    fun deviceId(): String
}

class SharedPreferencesDeviceIdentityStore(
    context: Context,
) : MobileDeviceIdentityStore {
    private val preferences = context.getSharedPreferences("termirust_mobile_device", Context.MODE_PRIVATE)

    override fun deviceId(): String {
        val existing = preferences.getString(KEY_DEVICE_ID, null)
        if (!existing.isNullOrBlank()) {
            return existing
        }
        val generated = "android-${UUID.randomUUID().toString().lowercase()}"
        preferences.edit().putString(KEY_DEVICE_ID, generated).apply()
        return generated
    }

    private companion object {
        const val KEY_DEVICE_ID = "device_id"
    }
}
