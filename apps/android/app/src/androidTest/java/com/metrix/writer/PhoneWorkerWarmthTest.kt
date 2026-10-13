@file:Suppress("DEPRECATION")
package com.metrix.writer

import android.content.ComponentName
import android.content.Context
import android.content.ContextWrapper
import android.content.ServiceConnection
import android.os.IBinder
import android.test.InstrumentationTestCase
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Binding lifetime only; never loads a model or starts inference. */
class PhoneWorkerWarmthTest : InstrumentationTestCase() {
    private fun connection() = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName, service: IBinder) {}
        override fun onServiceDisconnected(name: ComponentName) {}
    }
    fun testRetainsOnlyLatestIdleBindingAndExpiresIt() = verifyBindingLifetime()

    // Keep SAM-generated helper names away from JUnit 3's test* discovery.
    private fun verifyBindingLifetime() {
        val released = mutableListOf<ServiceConnection>(); val expired = CountDownLatch(1)
        val first = connection(); val second = connection()
        val app = object : ContextWrapper(instrumentation.targetContext) {
            override fun getApplicationContext(): Context = this
            override fun registerComponentCallbacks(callback: android.content.ComponentCallbacks) {}
            override fun unbindService(conn: ServiceConnection) {
                released.add(conn)
                if (conn === second) expired.countDown()
            }
        }
        try {
            instrumentation.runOnMainSync {
                PhoneWorkerWarmth.clear()
                PhoneWorkerWarmth.retain(app, first)
                assertTrue(released.isEmpty())
                PhoneWorkerWarmth.retain(app, second, 30)
                assertEquals(listOf(first), released)
            }
            assertTrue(expired.await(2, TimeUnit.SECONDS))
            instrumentation.runOnMainSync {
                PhoneWorkerWarmth.clear(); PhoneWorkerWarmth.clear()
                assertEquals(listOf(first, second), released)
            }
        } finally { instrumentation.runOnMainSync { PhoneWorkerWarmth.clear() } }
    }
}
