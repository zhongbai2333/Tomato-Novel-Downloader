plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.tomato.downloader"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.tomato.downloader"
        minSdk = 23
        // 关键：targetSdk=28 获得 Android W^X 豁免，filesDir 可执行下载的二进制
        // 这是 Termux 能运行下载二进制的核心原理（参见 termux-app/gradle.properties）
        // 代价：失去 scoped storage 等新特性，但我们不需要，且通过 sideload 安装
        targetSdk = 28
        versionCode = 7795
        versionName = "2.1.8"
    }

    // 签名配置：使用「林九思.jks」自签名证书，对 release 包进行 V1/V2 签名。
    // keystore 密码 = 别名 = "林九思"，与 key 密码一致。
    signingConfigs {
        create("release") {
            storeFile = file("../林九思.jks")
            storePassword = "林九思"
            keyAlias = "林九思"
            keyPassword = "林九思"
            // AGP 8.x 已弃用 enableV1Signing/enableV2Signing：改用 v1SigningEnabled/v2SigningEnabled
            // V1 + V2 双签名：确保 Android 7- (JAR) 与 Android 7+ (APK) 均能安装并做完整性校验
            isV1SigningEnabled = true
            isV2SigningEnabled = true
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
            signingConfig = signingConfigs.getByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
    }

    // 跳过 release 的 lint vital 检查（在低内存环境下避免 OOM，
    // 且本项目不依赖 lint 保证正确性，proguard 也未开启）
    lint {
        checkReleaseBuilds = false
        abortOnError = false
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.10.01")
    implementation(composeBom)

    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.navigation:navigation-compose:2.8.4")

    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")

    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")

    debugImplementation("androidx.compose.ui:ui-tooling")
}
