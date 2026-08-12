package com.tomato.downloader

import android.app.Application
import com.tomato.downloader.core.ConsentStore
import com.tomato.downloader.core.CustomWebStore
import com.tomato.downloader.core.PasswordStore
import com.tomato.downloader.core.PortStore
import com.tomato.downloader.core.ServerManager

class TomatoWebApp : Application() {
    override fun onCreate() {
        super.onCreate()
        ConsentStore.init(this)
        PasswordStore.init(this)
        PortStore.init(this)
        CustomWebStore.init(this)
        ServerManager.init(this)
    }
}
