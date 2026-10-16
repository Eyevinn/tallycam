package se.eyevinn.tallycam

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import se.eyevinn.tallycam.control.ProvisioningLink
import se.eyevinn.tallycam.ui.TallyCamScreen

class MainActivity : ComponentActivity() {
    private var model: AppModel? = null
    private var permitted by mutableStateOf(false)

    private val permissions = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        permitted = it[Manifest.permission.CAMERA] == true
        if (permitted) start()
    }

    /** The in-app QR scanner, for phones whose camera app does not open tallycam:// links. */
    private val scanner = registerForActivityResult(ScanContract()) { result ->
        val text = result.contents ?: return@registerForActivityResult
        val link = runCatching { android.net.Uri.parse(text) }.getOrNull()?.let(ProvisioningLink::parse)
        if (link != null) model?.provision(link) else model?.showMessage("Not a TallyCam code")
    }

    private fun scan() {
        scanner.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setBeepEnabled(false)
                           .setPrompt("Scan the QR code in the control hub").setOrientationLocked(false))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // A camera on a tripod must not lock itself.
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowInsetsControllerCompat(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
        val needed = arrayOf(Manifest.permission.CAMERA, Manifest.permission.RECORD_AUDIO)
        permitted = needed.all { checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }
        if (permitted) start() else permissions.launch(needed)
        setContent { TallyCamScreen(model, permitted, ::scan) }
    }

    private fun start() {
        if (model != null) return
        model = TallyCam.model(this)
        handle(intent)
        setContent { TallyCamScreen(model, permitted, ::scan) }
        // Android 13+: the streaming notification needs this; streaming does not.
        if (android.os.Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    /** tallycam://connect?... from the system camera's QR scanner. */
    private fun handle(intent: Intent?) {
        val uri = intent?.data ?: return
        ProvisioningLink.parse(uri)?.let { model?.provision(it) }
    }

    // The model lives as long as the process (TallyCam.model): a stream
    // goes on while the activity is gone, in StreamService's foreground.
}
