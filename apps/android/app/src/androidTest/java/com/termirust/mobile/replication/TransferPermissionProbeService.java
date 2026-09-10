package com.termirust.mobile.replication;

import android.app.Service;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.Message;
import android.os.Messenger;
import android.os.Process;
import android.os.RemoteException;
import java.io.InputStream;
import java.util.Arrays;

/** Standalone test APK process: uses Android APIs only, not the target app classloader. */
public final class TransferPermissionProbeService extends Service {
    @Override public IBinder onBind(Intent intent) {
        return new Messenger(new Handler(getMainLooper(), this::probe)).getBinder();
    }

    private boolean probe(Message message) {
        final String target = "com.termirust.mobile";
        try {
            if (message.sendingUid != getPackageManager().getApplicationInfo(target, 0).uid) return true;
        } catch (android.content.pm.PackageManager.NameNotFoundException error) { return true; }
        String location = message.getData().getString("uri");
        if (location == null) return true;
        Uri uri = Uri.parse(location);
        if (!"content".equals(uri.getScheme()) || !(target + ".replication-transfer-fixture").equals(uri.getAuthority())) return true;
        Bundle result = new Bundle();
        result.putInt("uid", Process.myUid());
        try (InputStream stream = getContentResolver().openInputStream(uri)) {
            if (stream == null) throw new java.io.IOException();
            byte[] bytes = new byte[18];
            int size = 0;
            while (size < bytes.length) {
                int count = stream.read(bytes, size, bytes.length - size);
                if (count == -1) break;
                if (count == 0) throw new java.io.IOException();
                size += count;
            }
            if (size > 17) throw new java.io.IOException();
            result.putString("status", "read");
            result.putByteArray("bytes", Arrays.copyOf(bytes, size));
        } catch (SecurityException error) { result.putString("status", "denied"); }
        catch (Exception error) { result.putString("status", "unavailable"); }
        if (message.replyTo != null) {
            Message reply = Message.obtain();
            reply.setData(result);
            try { message.replyTo.send(reply); } catch (RemoteException ignored) { }
        }
        return true;
    }
}
