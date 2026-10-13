package com.metrix.writer.account

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import androidx.credentials.ClearCredentialStateRequest
import androidx.credentials.CredentialManager
import androidx.credentials.CustomCredential
import androidx.credentials.GetCredentialRequest
import androidx.credentials.exceptions.ClearCredentialException
import androidx.credentials.exceptions.GetCredentialCancellationException
import androidx.credentials.exceptions.GetCredentialException
import androidx.credentials.exceptions.NoCredentialException
import com.google.android.gms.tasks.Task
import com.google.android.libraries.identity.googleid.GetSignInWithGoogleOption
import com.google.android.libraries.identity.googleid.GoogleIdTokenCredential
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseNetworkException
import com.google.firebase.FirebaseOptions
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.auth.FirebaseAuthException
import com.google.firebase.auth.FirebaseAuthInvalidUserException
import com.google.firebase.auth.FirebaseUser
import com.google.firebase.auth.GoogleAuthProvider
import com.metrix.writer.BuildConfig
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * Credential Manager's Google button flow for the configured web client audience, then
 * Firebase Authentication, as qualified by MW-OUATH-2. Firebase keeps its session in this
 * app's private storage, which Writer's backup and device-transfer rules exclude. It is
 * initialized only after someone chooses to sign in, so a guest never starts Firebase.
 * No token, UID, email or provider message is logged or returned in an error.
 */
internal class FirebaseGoogleIdentity(context: Context) : GoogleIdentity {
    private val context = context.applicationContext
    private val marker = this.context.getSharedPreferences("writer-account", Context.MODE_PRIVATE)
    private val webClientId = BuildConfig.FIREBASE_WEB_CLIENT_ID

    override val projectId = BuildConfig.FIREBASE_PROJECT_ID.takeIf {
        it.isNotEmpty() && BuildConfig.FIREBASE_APP_ID.isNotEmpty() && BuildConfig.FIREBASE_API_KEY.isNotEmpty() && webClientId.isNotEmpty()
    }

    private val auth by lazy {
        val app = FirebaseApp.getApps(this.context).find { it.name == APP_NAME } ?: FirebaseApp.initializeApp(this.context,
            FirebaseOptions.Builder().setProjectId(projectId!!).setApplicationId(BuildConfig.FIREBASE_APP_ID).setApiKey(BuildConfig.FIREBASE_API_KEY).build(), APP_NAME)
        FirebaseAuth.getInstance(app)
    }

    override fun restore(): IdentityUser? {
        // The marker is set before an exchange starts; without it Firebase is never touched.
        if (projectId == null || !marker.getBoolean(SESSION, false)) return null
        val user = auth.currentUser
        if (user == null || user.providerData.none { it.providerId == GoogleAuthProvider.PROVIDER_ID }) {
            signOutLocal()
            return null
        }
        return user.identity()
    }

    override suspend fun googleIdToken(context: Context): String {
        if (!online()) throw AuthFailure(AuthError.OFFLINE)
        val request = GetCredentialRequest.Builder().addCredentialOption(GetSignInWithGoogleOption.Builder(webClientId).build()).build()
        val credential = try { CredentialManager.create(context).getCredential(context, request).credential } catch (error: GetCredentialException) {
            throw AuthFailure(when (error) {
                is GetCredentialCancellationException -> AuthError.CANCELLED
                is NoCredentialException -> AuthError.PROVIDER_REJECTED
                else -> AuthError.UNAVAILABLE
            })
        }
        if (credential !is CustomCredential || credential.type != GoogleIdTokenCredential.TYPE_GOOGLE_ID_TOKEN_CREDENTIAL) throw AuthFailure(AuthError.INVALID_RESPONSE)
        return try { GoogleIdTokenCredential.createFrom(credential.data).idToken } catch (_: Exception) { throw AuthFailure(AuthError.INVALID_RESPONSE) }
    }

    override suspend fun signIn(googleIdToken: String): IdentityUser {
        marker.edit().putBoolean(SESSION, true).commit()
        val user = try { auth.signInWithCredential(GoogleAuthProvider.getCredential(googleIdToken, null)).await().user } catch (error: Exception) {
            if (auth.currentUser == null) marker.edit().putBoolean(SESSION, false).commit()
            throw failure(error, AuthError.PROVIDER_REJECTED)
        }
        return user?.identity() ?: throw AuthFailure(AuthError.INVALID_RESPONSE)
    }

    override suspend fun reauthenticate(googleIdToken: String, uid: String): IdentityUser {
        val user = auth.currentUser?.takeIf { it.uid == uid } ?: throw AuthFailure(AuthError.REAUTH_REQUIRED)
        // Firebase rejects a credential for another account here without replacing this one.
        try { user.reauthenticate(GoogleAuthProvider.getCredential(googleIdToken, null)).await() } catch (error: Exception) {
            throw failure(error, AuthError.PROVIDER_REJECTED)
        }
        return user.identity()
    }

    override suspend fun idToken(uid: String, forceRefresh: Boolean): String = tokenResult(uid, forceRefresh).first

    override suspend fun freshIdToken(uid: String): FreshToken {
        val (token, authTime) = tokenResult(uid, true)
        if (authTime <= 0) throw AuthFailure(AuthError.INVALID_RESPONSE)
        return FreshToken(token, authTime)
    }

    private suspend fun tokenResult(uid: String, forceRefresh: Boolean): Pair<String, Long> {
        val user = auth.currentUser?.takeIf { it.uid == uid } ?: throw AuthFailure(AuthError.REAUTH_REQUIRED)
        val result = try { user.getIdToken(forceRefresh).await() } catch (error: Exception) {
            throw failure(error, if (error is FirebaseAuthInvalidUserException) AuthError.REAUTH_REQUIRED else AuthError.PROVIDER_REJECTED)
        }
        val token = result.token
        if (token.isNullOrEmpty() || auth.currentUser?.uid != uid) throw AuthFailure(AuthError.INVALID_RESPONSE)
        return token to result.authTimestamp
    }

    override fun signOutLocal() {
        if (marker.getBoolean(SESSION, false) || FirebaseApp.getApps(context).any { it.name == APP_NAME }) auth.signOut()
        marker.edit().putBoolean(SESSION, false).commit()
    }

    override suspend fun clearCredentialState() {
        try { CredentialManager.create(context).clearCredentialState(ClearCredentialStateRequest()) } catch (_: ClearCredentialException) {}
    }

    private fun online(): Boolean {
        val manager = context.getSystemService(ConnectivityManager::class.java) ?: return true
        val capabilities = manager.getNetworkCapabilities(manager.activeNetwork) ?: return false
        return capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }

    private fun failure(error: Exception, otherwise: AuthError) = AuthFailure(when (error) {
        is AuthFailure -> error.kind
        is FirebaseNetworkException -> AuthError.OFFLINE
        is FirebaseAuthException -> otherwise
        else -> AuthError.UNAVAILABLE
    })

    private fun FirebaseUser.identity() = IdentityUser(uid, email, displayName)

    private suspend fun <T> Task<T>.await(): T = suspendCancellableCoroutine { continuation ->
        addOnCompleteListener { task ->
            val error = task.exception
            if (error != null) continuation.resumeWithException(error)
            else if (task.isCanceled) continuation.resumeWithException(AuthFailure(AuthError.UNAVAILABLE))
            else continuation.resume(task.result)
        }
    }

    private companion object {
        const val APP_NAME = "writer-account"
        const val SESSION = "firebase_session"
    }
}
